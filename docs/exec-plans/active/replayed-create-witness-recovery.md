# 重放 create journal 证据恢复

- 状态：本地实现与验证完成；生产部署验收待主任务安排
- 负责人：Codex Luna
- 创建日期：2026-09-11

## 目标

为 `file_committed` 的 create journal 增加受强证据约束的 `superseded` 终态，安全处理 canonical 文件和后续元数据已经由另一个 File ID 接管的恢复冲突。

## 范围与边界

包含 State 的事务性 witness verifier、`operation_journal` 前向迁移、Core 恢复分支、恢复计数和本地 fixture。验证只使用本地 SQLite 与临时 Vault。

不包含生产数据库修复、自动修改 Vault 状态、删除 canonical 文件、改写后续 revision/history/outbox，或把一般的并发冲突纳入自动合并。

## 实施步骤

1. 扩展 journal 状态解析和数据库约束，保留原迁移及现有 idempotency 索引。
2. 在 State repository 内实现 Vault 作用域、事务内重读、CAS 更新和审计写入。
3. 在 Core 的 `AlreadyExists` 恢复分支调用 witness verifier；证明成功计入 `superseded`，否则保持 `needs_review`。
4. 增加精确成功、每类证据缺失、跨 Vault、哈希变化、幂等和迁移重启 fixture。
5. 更新 schema 文档和相关迁移版本断言，执行格式化、State/Vault Core 测试及文档检查。

## 验收证据

- 原 journal 只在所有 witness 条件满足时变为 `superseded`。
- 成功路径不插入原 File ID 的 revision，不改 replacement 条目，不产生第二条 audit。
- 失败路径仍可被维护审查识别，且第二个 prepared journal 可正常恢复。
- 所有查询带当前 `VaultContext` 的 Vault 谓词。

已完成的本地证据：State witness fixture 通过真实 `source_path`／`temp_path` 形态、哈希和 replacement ID 负例、reviewed 显式入口与审计幂等；非终态 journal 只要 `source_path` 或 `destination_path` 指向 witness 路径就会阻止接管。Vault Core 真实 staged PUT 在 `OutboxInserted` 故障后通过恢复进入 `superseded`；Memory initialization service 通过 A/B/C 后续 claim、另一个 prepared 操作、普通内容和新 v3 内容保留、Vault `Error` 状态保留以及后台恢复到 `ready`。诊断字段在 SQLite 关闭并重新打开后保留，requeue 时清除旧诊断。

完整本地验证已通过：

| 命令 | 结果 |
|---|---|
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过，exit 0 |
| `cargo test --workspace --all-features` | 通过，exit 0；所有单元、集成及文档测试无失败 |
| `cd frontend/admin && ./node_modules/.bin/eslint .` | 通过，exit 0 |
| `cd frontend/admin && ./node_modules/.bin/vitest run` | 通过，3 个测试文件、42 项测试 |
| `cd frontend/admin && ./node_modules/.bin/tsc --noEmit` | 通过，exit 0 |
| `cd frontend/admin && ./node_modules/.bin/vite build` | 通过，exit 0 |

以上均为本地 SQLite、临时 Vault 和现有离线前端依赖验证。没有连接或修改生产环境；生产部署及部署后验收由主任务统一安排。

## 风险与恢复

新终态只会减少自动重试，不会改变 canonical 内容。若 witness 不完整，系统保留 `needs_review`。迁移失败则按 SQLx forward-only 规则停止启动，不修改既有迁移文件。
