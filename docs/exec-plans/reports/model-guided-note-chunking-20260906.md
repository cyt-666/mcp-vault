# 模型分块与诊断退出检索门槛：验收报告

日期：2026-09-06。基线 HEAD `6500214` 加本地未提交修复，版本 0.2.2。
用户明确批准修改方向；[ADR-0028](../../adr/0028-model-guided-chunks-and-diagnostic-evaluation.md)
和[ExecPlan](../completed/model-guided-note-chunking.md)取代此前自动校准必须通过的要求。

## 已实现

- 内置评测仅为可选诊断。启动/普通配置补偿不再提交合成请求，缺失/失败报告不禁用语义检索。
  当前模型、合法向量、来源资格、Vault 隔离、对象去重、响应预算仍强制检查。
  相似度用于候选排序，不代表资料一定回答问题；没有把旧 quality_failed 改成 passed。
- Admin 实际模型绑定 API/UI 新增 `note_chunking`。模型只分组连续原文单元，程序使用既有
  Markdown 结构生成的纯文本投影，不采用模型改写内容。校验完整覆盖、顺序、无重复和大小。
- 分组、规则回退和短期认领存入 schema 0019。排队、向量源解析、查询章节及状态读取同一计划。
  查询不调用生成；未改来源重复处理、重建索引、重开磁盘 DB 都复用结果。
- 已有有效规则向量不因启用新角色而失效。模型失败、非法分组、超限输入使用规则回退并保存，
  不在每次扫描重试。过期来源不可读取/发布旧计划；并发认领和故障后回退避免重复消费。
- 索引页面显示已保存的模型分组、保留/规则回退和正在分组数量。诊断页面明确手动调用的费用和局限。

## 大小与空配置默认值

| 项目 | 强制约束 |
|---|---|
| 单块向量文本 | 最多 2048 UTF-8 字节，包含最多 512 字节标题上下文及分隔符；分组正文最多 1534 字节 |
| 单次模型分组 | 原文投影最多 32 KiB、128 个单元；输出最多 2048 tokens，整次调用 60 秒 |
| 分组上下文 | 配置值或默认 8192；按输入 UTF-8 字节 + 输出额度 + 512 包装预留保守检查，不是精确 tokenizer |
| embedding 输入 | context_window 未填时每条最多 8192 字节；填入时按该数值保守限制字节，笔记块仍受较小的 2048 字节上限 |
| embedding 维度 | 已填 dimension 必须精确匹配；未填仍校验形状，并设 8192 维资源上限。上限不是要求输出该维度 |

现有 Admin dimension 字段实际是输出校验，已更名说明；不会发送厂商 dimensions 参数，
不会自动选择最大维度，也不会截断向量。窗口/维度的默认资源约束不是对模型最佳效果的判断。
大于分组输入范围或包含无法整组容纳的超长单元时使用有界规则，不宣称所有笔记都由模型分组。

## 测试证据

日志位于 `target/memory-review/`，仅本地合成数据，未追加真实收费请求。

| 命令/场景 | 结果 |
|---|---|
| `cargo test --offline --locked --workspace` | 325 passed，0 failed/ignored；`chunking-workspace-final.log` |
| `cargo test --offline --locked --workspace --all-features` | 325 passed，0 failed/ignored；`chunking-all-features.log`，使用下述隔离 ORT |
| `cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings` | exit 0；`chunking-clippy-final.log` |
| 前端已安装 ESLint / Vitest / TypeScript / Vite | 全通过，35 tests；`chunking-frontend-{lint,test,typecheck,build}.log`；pnpm 主机包装器问题未修改依赖来掩盖 |
| `python scripts/e2e/memory-admin.py` | Chromium + 实际 Admin HTTP：U1–U5、分页、报告宽度、新分块角色绑定通过；`chunking-browser/result.json` |
| `CARGO_NET_OFFLINE=true bash scripts/release/check-migrations.sh` | exit 0，含 18→19、预算/报告保留与两 Vault 自动/手动任务隔离；`chunking-migrations.log` |
| `cargo test --offline --locked -p mcp-vault-indexer chunk_plan` | 2 passed；最终增强磁盘重开、真实来源修改后旧计划不可发布；`chunk-plan-tests-final.log` |
| fmt / docs / diff whitespace | `cargo fmt --all --check`、`bash scripts/check-docs.sh`、`git diff --check` |

新 A01 用既有磁盘库与向量启动真实 reconciliation/worker 两次，证明零自动诊断调用且无需报告即可
语义 recall，业务向量/记忆身份保留。模型分组合同通过真实 HTTP adapter，随后实际生成向量并查询
匹配章节；返回重复单元时保存规则回退且下一轮零额外生成。边界单测覆盖缺失、重复、乱序、越界、
超长组和配置为空时的默认输入/维度拒绝。测试不是模型分组质量评分；真实效果对比仍未执行。

## 环境跟进

已将官方 `onnxruntime==1.24.2` wheel 的动态库隔离解包到
`target/memory-review/followup-tools`，不改系统库。全特性测试/Clippy 使用：

```bash
ORT_LIB_LOCATION=/home/cheng/code/mcp-vault/target/memory-review/followup-tools
ORT_PREFER_DYNAMIC_LINK=1
LD_LIBRARY_PATH=/home/cheng/code/mcp-vault/target/memory-review/followup-tools
```

以上为各命令前导环境变量；目录包含 `libonnxruntime.so`、`.so.1` 指向实际 `.so.1.24.2`。
默认静态 ORT 包在本机仍有 ABI 不兼容，使用匹配动态库的验证已通过；未禁用 fastembed 特性。

Litmus 也已隔离安装并实际运行，修复了仓库 wrapper 把 suite 错作为第四参数的问题（应使用 TESTS）。
结果 basic 16/16、copymove 13/13、http 4/4；props 10/14、locks 36/41。
未通过项涉及 PROPPATCH 501、PROPFIND href 和复杂 If 请求连接关闭，日志在
`system-audit/litmus-*.log`，这是现存 WebDAV 兼容性问题，不能算全通过。当前用户批准的分块改动没有
冒充修复这些独立问题；实际 Obsidian/完整 Docker 镜像验收仍需单独完成。

## 升级与限制

0019 是追加迁移，保存边界而不改 canonical Markdown，取消旧自动诊断而不退款或改写旧结果。
本地更新前保留 `local-before-0019.sqlite3` 一致性备份（0600）。没有替用户绑定 note_chunking，
没有实际生成新分组或新增收费调用。现有已检查的真实 embedding 缓存仍为诊断失败记录，
但不再影响生产检索；不声称改策略提高了实际精确率。

升级/回滚按[操作说明](../../memory-autocalibration-operations.md#upgrade-and-rollback)：
协调备份 DB、Vault、历史和密钥；回滚用匹配旧 schema 的整套备份，勿直接用旧 SQLx 二进制读新 schema。
本轮未 commit、push 或生产部署，保留此前用户修改及所有回归修复。

本地升级实测：schema 18→19，readiness 正常；校准请求累计 26→26，模型绑定、
历史报告和缓存指纹完全相同，分组计划 0 条（未替用户绑定模型）。
对比证据：`chunking-upgrade-before.json`、`chunking-upgrade-after.json`。
首次直接执行启动脚本因无执行位失败，随后通过 `sh` 正常启动；健康检查纠正到
实际 `/health/ready`，不是不存在的 `/health`。
