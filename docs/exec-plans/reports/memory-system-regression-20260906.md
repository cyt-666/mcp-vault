# 记忆系统两份计划综合回归报告

后续用户已批准取消生产评测门槛并增加模型分块，见[新验收报告](model-guided-note-chunking-20260906.md)。以下保留改方向之前的结果。

日期：2026-09-06。基线 HEAD `6500214`，测试对象包含其后的本地未提交修复；版本仍为 0.2.2。
依据：[修复计划](../active/memory-review-autocalibration-admin-fix.md)、[记忆优化计划](../completed/memory-simplification-and-retrieval-v2.1.md)、ADR-0026/0027。
此前诊断与 Provider embedding 响应 index 修复保留。没有使用生产 Vault、push 或部署。

## 结论和新发现

工程回归通过不等于真实语义质量通过。本轮新复现并修复三项问题：

| 问题 | 修复和可复核证据 |
|---|---|
| 更换模型后旧校准任务已结束，记录仍显示 running | worker 仅终结本任务拥有、同 Vault 的过期签名；保留缓存、预算和已完成报告。`obsolete_calibration_job_retires_only_its_vaults_unfinished_profiles` 先失败后通过。0018 同时修复已有孤立记录，排除仍有活动任务的记录。 |
| 仅改 Provider 显示名或原样保存模型也让有效向量失效 | 区分 Admin 编辑 revision 和 embedding identity revision；CAS 仍拒绝旧编辑，endpoint/维度等实际变化仍失效。`renaming_provider_should_preserve_valid_embedding_identity` 先失败后通过。0018 从旧 revision 初始化身份，保留原指纹。 |
| 展开校准原始报告撑宽整页 | 报告使用可换行的 data-inspector，单列网格允许收缩；真实浏览器新增整页宽度断言，复现后修复。 |

没有恢复历史状态机或全局 consolidation；没有借校准迁移旧知识、解除暂停、重新提取全库或放松质量门槛。

## 需求与实际回归覆盖

所有下列 Rust 用例都在本轮默认特性完整工作区测试中重新执行。具体逐项测试名可同时查阅
[原验收报告](memory-review-autocalibration-admin-fix.md)和优化计划的 T01–T28 表；这里记录本轮证据范围。

| 计划范围 | 本轮证据与限制 |
|---|---|
| R1–R6 | 语义过滤、双通道正余弦负例、无原 Provider 删除、context-only 候选、1→32 重复块名次不变、完整 JSON/长标题预算全部通过；包括最终返回前删除竞态。 |
| A01 | `a01_existing_binding_and_vectors_calibrate_on_start_without_admin_or_regeneration`：磁盘库重开、真实启动补偿与 worker、HTTP adapter、计算和发布，保留有效业务向量，再次启动不追加请求。端点为本地合成模型；不是旧生产二进制升级或真实模型质量证明。 |
| A02–A08、A12–A16 | 去重、重启预算/缓存、角色独立、暂停/禁用、签名变化、发布恢复与 Vault 隔离；本轮增加过期状态和稳定身份回归。 |
| A09–A11、A17 | 失败不能发布、内部评测不依赖已启用语义、两通道无答案失败集合联合门槛；真实 embedding 失败结果也如实保留。 |
| U1–U5 | Chromium 通过实际 Admin HTTP 完成预检、确认迁移、校准运行与维护、空暂停来源恢复、显式记忆编辑；临时 SQLite/Vault，只有模型响应为合成。 |
| U6 | 浏览器 53 条记忆分页；35 项前端测试含 51 个暂停来源、轮询尾页、迟到响应与 Vault 切换隔离；新增报告展开宽度检查。 |
| G1/G2 | 15 篇合成来源实际生成、逐事实核对，见下文；独立人工复核仍 pending。 |
| G3 | 合法空集合、截断、非法字段、来源变化和超限失败边界通过，不能用失败清空来源。 |
| G4 | 章节/snippet/revision/投影偏移和语义路径合同测试通过；真实模型当前质量门槛失败，真实公开语义效果未验收。 |
| S1–S7 | 多 Vault、旧 ID/向量不可见、快照及来源恢复、0015–0018 兼容、Origin/CSRF、完整预算、校准与业务知识隔离通过。 |
| T01–T05 | 余弦准入、词面降级、显式记忆直接写入/更新/真实删除、可选字段保留与清空。 |
| T06–T09 | 单来源集合替换、独立归属、严格结构、否定/条件；合同通过，真实生成结论限于下述样本。 |
| T10–T16 | 词面负例、跨语言元数据、当前 input/profile 资格、坏向量拒绝、唯一对象排序。真实跨语言整体质量不能由这些合同用例代替。 |
| T17–T22 | Markdown/章节投影、超过 128 块的覆盖与分页、长记忆首中尾、完整响应预算和来源诊断。 |
| T23–T28 | 来源移动/删除/重建/暂停、故障恢复、淘汰旧任务、Vault 隔离、秘密和结构边界、明确预检的旧记忆迁移。 |

## 授权的真实生成：15 次，已结束

用户明确允许最多 15 次，并在隔离本地页面绑定 `mimo-v2.5`。新 evaluator 通过实际
ProviderService → MemoryService → Vault Core 路径运行已有 `generation-coverage.json`；
源配置只读，复制必要加密配置至临时隔离库，合成笔记不写入用户 default Vault。
预算在每次 HTTP dispatch 前持久记录，禁用内部重试；输出目录不得复用。另有本地 503 合同测试
`generation_budget_blocks_retries_and_cloned_service_dispatch` 证明预算限制覆盖重试和服务克隆。

可复现命令（本轮已完成，不应再次运行以追加消费）：

```bash
cargo run --offline --locked -p mcp-vault-memory --example real_generation_eval -- --source-data target/memory-review/local-model.09VRlm/data --output target/memory-review/system-audit/real-generation --authorized-requests 15
```

实际 15 次请求、15 篇全部提取成功、132 条记忆；相同来源再次提取 15/15 跳过且无额外请求。
逐项对照得到 91/91 个标注事实有输出证据，15/15 保留主语言；未观察到将否定或未采纳建议反转为已完成。
但第 13 篇的第 4、7 条把完成状态和尚未开始的阶段关联到 study_period 的日期范围/结束时间；
原文没有明确逐项绑定这些时间。属于需要复核的时间推断，不能当作完全确定的原文事实。

证据为 `target/memory-review/system-audit/real-generation/` 下的 15 份逐例输出、
`requests.jsonl`、`report.json` 和带逐事实引用的 `source-output-review.json`。
复核者是 Codex，**不是盲测或独立人工评分**；未追加模型裁判调用，未编造 support precision。
15 个模板化合成来源不能代表私人 Vault 的全体质量；人工复核仍 pending。

## 真实 embedding：排序正确，当前准入仍失败

复用此前用户触发的本地 `embedding-3` 缓存，只读重算，没有重发模型请求。
两通道在 holdout 的 40 个有答案问题均将正确文档排第一，原始 Recall@5 为 1.0。

| 通道 | 校准集选出的余弦门槛 | holdout 结果 | 判定 |
|---|---|---|---|
| memory | 0.5099286437 | 找到 37/40；纯语义 17/20；无答案误返 4/20（20%） | 超过 5% 误返上限，失败 |
| note | 0.4427765161 | 找到 27/40；纯语义 7/20；无答案误返 1/20 | 召回、精确率与纯语义未达标，失败 |

说明当前校准集选出的统一门槛未能在 holdout 达到要求，不能归结为模型没排对答案。
也没有证明所有可能的策略都无法通过。原生产报告的 0.999999 回退原因仍未确认：
未读取其缓存，本地两次完成评测都找到了校准集可用门槛，与该报告不同。
没有针对已查看的 holdout 调参后冒充独立验收，也没有强制启用语义。
缓存诊断证据见 `target/memory-review/local-model.09VRlm/{memory,note}-diagnostics.json` 和 `findings.md`。

## 实际命令、结果与环境限制

本轮日志统一在 `target/memory-review/system-audit/`（忽略目录，不提交运行数据或秘密）。

| 命令/检查 | 结果与日志 |
|---|---|
| `cargo fmt --all --check`、`bash scripts/check-docs.sh`、`git diff --check` | 全部 exit 0；`fmt-final.log`、`docs-final.log`、`diff-final.log` |
| `cargo test --offline --locked --workspace` | 321 passed，0 failed/ignored；`workspace-final.log` |
| `cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings` | exit 0；`clippy-final.log` |
| `cargo test --offline --locked --workspace --all-features` | exit 101；`all-features.log`。主机 ORT 需要缺失的 `__isoc23_strtol/strtoll/strtoull`、`basic_string::_M_replace_cold`，不是测试通过。 |
| `pnpm --dir frontend/admin lint` / `test` / `build` | 主机 pnpm 自动安装/store SQLite 报 `unable to open database file`；三个 `frontend-*.log` 保留失败。 |
| 在 `frontend/admin` 执行 `./node_modules/.bin/eslint .`、`vitest run`、`tsc --noEmit`、`vite build`（后三者同一 bin 前缀） | 使用已安装依赖执行相同脚本，全部 exit 0，35 tests；`frontend-{lint,test,typecheck,build}-final.log` |
| `bash scripts/release/check-migrations.sh` | exit 0，含 0018；`migrations-final.log` |
| `CARGO_NET_OFFLINE=true bash scripts/interop/http-smoke.sh` | exit 0；`http-smoke.log`，实际 HTTP、并发 DAV 写入/前置条件、Admin 隔离、OAuth/MCP。 |
| `python scripts/e2e/memory-admin.py` | 真实 Chromium/Admin 流程；报告展开先红后绿，`browser-overflow-{red,green}.log` 与同名输出目录。使用本地已安装 Chromium 和隔离 fixture。 |
| `cargo run --offline --locked -p mcp-vault-memory --example quality_eval -- --mode deterministic --fixtures tests/fixtures/memory-quality --output target/memory-review/system-audit/deterministic-quality.json` | exit 0；合成合同结果，不是真实质量评分。 |
| 官方 MCP conformance | 固定上游 SHA `74edef34d674f563537be8c6587cebaa58e830ca` 的本地包，4 个版本均通过既有 baseline，无新增例外；`mcp-2026-07-28.log`、`mcp-2025-11-25.log`、`mcp-2025-06-18.log`、`mcp-2025-03-26.log`。 |
| WebDAV Litmus | 可执行文件未安装；`litmus-preflight.log` 保留 exit 2 原因。不能算通过。 |

MCP 使用 `CARGO_NET_OFFLINE=true npm_config_offline=true`、`MCP_VAULT_CONFORMANCE_PACKAGE=file:` 指向上述固定本地包，
通过 `MCP_VAULT_CONFORMANCE_SPEC_VERSION=<version> bash scripts/conformance/mcp.sh` 按上述四个版本运行。上游工具不接受 2024-11-05，先前证据仍为 pending；未下载替代版本或增加忽略项。
实际 Obsidian 客户端手工兼容验收未执行。完整 Docker 构建仍受主机 Docker socket 权限限制；
先前缺失 corpus 的隔离构建输入回归已通过，不能据此声称完整新镜像验收通过。

## 兼容升级、本地验证和回滚

0018 为新增迁移，既有迁移未改。测试使用 0017 的真实表结构，验证旧 revision 7/11 初始化后保持原身份，
实际向量 bytes/时间、缓存、已完成报告和累计预算均不变；孤立旧记录关闭，活动记录和其他 Vault 不受影响。

用户配置模型的隔离本地服务已更新到当前后端和 schema 18：
升级前后校准请求总计均为 **26**，所有缓存和已完成报告不变，旧悬挂记录关闭，注册笔记数仍为 0；
登录和模型配置保留。`local-upgrade-before.json` / `local-upgrade-after.json` 保存对比，
`local-before-0018.sqlite3` 为权限 0600 的一致性升级前备份。真实生成的 15 次使用独立临时实例，不属于这 26 次。

生产升级仍需协调备份数据库、Vault、历史和密钥；回滚恢复匹配旧二进制的整套备份，不能把旧 SQLx 二进制直接指向新 schema。
详见[升级和回滚说明](../../memory-autocalibration-operations.md#upgrade-and-rollback)。本轮未提交、推送或部署生产。
