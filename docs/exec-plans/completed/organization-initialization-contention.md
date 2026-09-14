# ExecPlan：部署后记忆整理失败与超时修复

## 状态与目标

创建／更新：2026-09-07。执行者：Codex。状态：本地工程验收完成；生产部署效果尚未验证。

用户已经部署增量整理，要求根据截图与日志修复失败。结果应是初始化分页保存进度，内部记忆文件写入不会放大全库索引工作，语义判断不会被先前请求消耗的批次预算截断，并能区分请求超时与本地状态错误。

## 约束依据与仓库现状

遵守 [ADR-0031](../../adr/0031-incremental-memory-organization.md)、[记忆规范](../../memory-system.md#initialization-write-pressure-and-operational-diagnostics)、[架构](../../architecture.md)、[数据模型](../../data-model.md#migration-0025-bounded-initialization-checkpoints)、[接口](../../interfaces.md) 和 [安全要求](../../security.md)。协议只调用服务，SQL 留在仓库层，规范文件通过 Vault Core 写入。生产已执行迁移 0024，不能改写已部署迁移。

本轮开始时，增量整理已在工作区实现。`organization.rs` 负责初始化与语义判断，State 保存来源、条目和正式发布操作，Server 处理事件和任务续跑。旧实现初始化全量遍历；语义阶段每轮最多处理 8 条，却让所有请求共用 300 秒截止时间。

## 证据与发现

第一张截图处于初始化、模型调用为 0，错误为 `memory_state_error`。用户提供的 100 行日志包含同一任务运行 87.6 秒后的 `memory_core_error`、38 条耗时 1～4.524 秒的 SQL 警告、8 次索引任务启动与 11 次向量任务启动。日志没有底层异常，因此不能宣称 SQLite busy 或任何特定约束已被证实为生产根因。

`outbox_to_job_handler` 对内部文件写入屏蔽来源提取，却仍为每个事件创建 `index.rebuild`，后者执行全 Vault 重建及笔记向量维护。回归先复现 3 次内部写入加 1 次普通写入产生 4 个重建任务，修复后只产生 1 个。旧内部任务若参与合并判定，还会错误地吞掉普通笔记重建，需要同时修复入队、执行和合并判定。

用户随后报告 `memory_organization_slice_timeout`，最新截图明确处于语义整理：已处理 95/325、待处理 230、模型调用 30、尝试 9/10。它不能用“仍在初始化”解释。代码证实整轮 300 秒预算覆盖多次模型判断，而单次请求默认也是 300 秒，后续请求可能被剩余批次时间提前取消。截图不能给出某次实际请求的精确耗时，但足以纠正阶段判断。

## 设计与范围

1. Server 排除保留目录文件的普通索引任务，并在执行入口跳过旧内部任务。State 合并判定使用相同路径策略、Vault 条件及每页 128 条游标，忽略会被跳过的任务。普通笔记和手动索引保持可运行。
2. Memory 跳过内容未变化的旧独立条目，避免无意义规范文件重写。前向迁移 0025 为初始化检查和接管增加独立游标与计数；每阶段每轮最多 8 条，完成发布或验证无变化后推进，重启先恢复正式发布操作。
3. 一轮最多执行一次未缓存的逻辑模型判断；无候选／缓存命中的条目仍可最多处理 8 条。逻辑请求独立使用配置超时，覆盖 Provider 并发门与重试；外层为 300 秒本地余量加一次配置请求预算。初始化另有 300 秒上限。剩余时间不足以容纳完整请求与 30 秒发布余量时正常续跑。
4. State／Memory 输出静态脱敏错误分类，Server 记录实际失败阶段。页面区分初始化进度、批次超时和逻辑请求超时。正常检查点使用 `Deferred`，不消耗失败重试次数。

本轮不提交、推送或部署，不清除当前来源贡献、既有整理检查点、规范文件、审计或正式操作日志；不使用真实付费模型作回归。

## 进度

- [x] 2026-09-07：复现并修复内部事件放大全库索引及旧任务合并问题，包括大小写路径策略。
- [x] 2026-09-07：补充静态状态／Core 分类、失败阶段和 UI 提示；验证真实 SQLite 竞争及唯一约束分类。
- [x] 2026-09-07：迁移 0025 与初始化分页；验证 25 条数据跨服务重启保存检查、接管进度，以及 migration-24 升级保留初始化、暂停和任务状态。
- [x] 2026-09-07：一次逻辑判断后保存进度；回归验证第一轮 1 次调用／1 条完成，第二轮 2 次累计调用／2 条完成，最终工作收敛；验证剩余时间不足时拒绝开始请求并正常续跑。
- [x] 2026-09-07：前端 lint、38 项测试和构建通过。
- [x] 2026-09-07：最终格式检查、工作区 Clippy 和全部 378 项 Rust 测试通过，0 失败、0 忽略。
- [x] 2026-09-07：汇总[验收报告](../reports/organization-incident-fixes-20260907.md)并归档计划；生产部署验证单独列明。

## 验证

输出保存在 `target/memory-organization-validation/`：

- `cargo fmt --all --check` → `incident-final-fmt.log`。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` → `incident-final-clippy.log`。
- `cargo test --workspace --all-features` → `incident-final-tests.log`，协议测试需要本地监听权限。
- `CI=true pnpm --dir frontend/admin lint` → `incident-final-frontend-lint.log`。
- `CI=true pnpm --dir frontend/admin test` → `incident-final-frontend-test.log`。
- `CI=true pnpm --dir frontend/admin build` → `incident-final-frontend-build.log`。

已有定向证据：`managed-events-red.log`、`managed-events-green.log`、`managed-events-final.log`、`semantic-slice.log`。完整回归覆盖迁移、Vault 隔离、发布恢复、Provider 替身、协议路由与普通笔记索引。模型实际质量与生产时延不由本地替身证明。

## 风险、回滚与恢复

迁移 0025 只添加检查点列，保留已初始化状态；已经进入语义阶段的实例不会重复切换。任务失败或进程重启继续持久待处理条目，未完成正式发布先恢复。迁移无逆向脚本；完整回滚按既有升级流程一起恢复数据库与 Vault 备份。新请求边界减少每轮调用数，但允许既有 Provider 重试策略，整体吞吐取决于模型时延。

## 结果

本地修复完成，六项工程门禁全部通过。初始化分批、语义单次判断后保存检查点、前向迁移和索引工作放大修复均有回归覆盖。需要部署包含本次修复的新二进制后才能验证生产状态；不宣称生产故障已解除。
