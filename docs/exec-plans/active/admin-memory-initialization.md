# Admin 一键初始化旧记忆清理

- 状态：实现完成，生产验收待执行
- 负责人：Codex Luna
- 创建日期：2026-09-11
- 最近更新：2026-09-11

## 目标与用户可见结果

当服务启动发现某个 Vault 的 memory v3 初始化处于 `required` 或 `clearing` 时，Admin 页面显示受 Vault 隔离的初始化状态和只读预览。管理员通过认证、Origin/CSRF 保护的明确确认按钮启动后台任务，任务立即返回 HTTP 202，在服务内部安全地完成旧 memory 数据和受管旧文件清理，同时保留普通 Vault 内容和新 v3 memory 数据。页面持续显示持久化阶段、文件进度、失败安全错误码，并支持服务重启后的查看和显式续跑。ready 状态保持幂等，不触碰新 v3 数据。

## 约束依据

- `docs/adr/0033-source-preserving-memory-units.md`：旧记忆仅在明确的一次性初始化中清理；普通启动不清零；新 v3 数据和普通 Vault 内容必须保留。
- `docs/exec-plans/active/memory-system-v3.md`：初始化、暂停 generation、恢复和跨 Vault 隔离要求。
- `AGENTS.md`：协议层不执行业务逻辑，所有写入经过 Vault Core，维护状态与安全恢复必须可持久化。
- `crates/domain/src/maintenance.rs`：进程级 MaintenanceGate 负责 admission 和 drain。
- 现有 Backup/Restore 维护服务：初始化、备份和恢复共享独占维护 lease。

## 当前状态

现有 CLI `initialize-memory --discard-legacy-memory` 已有预览、manifest、checkpoint、恢复和安全诊断，但要求离线 exclusive store，Admin 没有在线调度入口。Admin 已有认证、Origin/CSRF、Vault 选择、generation status 和 Backup 维护路由。普通 workers 通过 MaintenanceGate admission 执行工作，BackupService 已持有维护状态，但 gate 尚无带有限时 drain 的统一异步 lease。

## 范围

包含：

1. 为 MaintenanceGate 增加共享的有限时 Offline 独占 lease；进入时切换 Offline、等待既有 operation/write admission drain，超时恢复原模式并返回失败；Backup/Restore/Recover/Memory Init 共享该 lease。
2. 为 memory v3 增加持久化初始化任务状态、预览和后台执行入口。任务使用现有 manifest/checkpoint，禁止普通 job handler 抢占控制任务；旧 memory journal 仅在显式确认后、严格证明路径全在 legacy namespace 时终结为 `discarded`，不重放。
3. 增加 Admin Vault-scoped 预览、确认启动、状态和显式续跑路由，保留认证、Origin、CSRF 及当前 Vault 隔离。
4. 更新 Admin UI，展示旧记录/受管文件数量、保留范围、确认清理、进度、失败错误和续跑按钮。
5. 增加迁移、服务/API/UI 测试和临时 fixture 验证；更新 ADR 与执行文档。

不包含：

- Provider 调用或新的记忆提取策略。
- 通用任务平台重构。
- 自动清理旧数据或绕过 Admin 确认。
- 放宽 CLI 的 offline exclusive 保护。
- 生产数据库、真实 Vault 或付费 Provider 操作。

## 设计

### 维护 lease

在 `MaintenanceGate` 上实现受控的 `try_begin_offline` 入口和独占所有权，返回 RAII lease。入口先原子切换 Offline，等待 active operations 和 writes 归零；请求本身不先持有普通 operation guard。仅在尚未开始数据变更且 drain/setup 失败时恢复进入前的模式；已经开始清理或恢复的失败保持 Offline，交由显式维护恢复路径判断完整性后重新开放。lease 期间只允许 Admin 登录、身份会话、必要 Vault 列表、初始化预览/状态/确认/续跑和维护恢复路径；数据平面和普通 Admin 写路由继续被 admission middleware 拒绝。Backup/Restore/Recover/Init 均通过同一 lease，不能只写 mode。

### 持久化任务

扩展 memory initialization 状态表保存任务阶段（preview、queued、draining、clearing、failed、ready）、task id、requested/started/finished 时间、completed/total 文件数、last error code、resumable 标志和 generation paused 状态。现有 manifest 是旧文件清理的权威持久清单，checkpoint 逐文件更新。创建任务在事务中校验 required/clearing 状态和确认 token；任务启动后再次读取状态，ready 直接幂等返回，failed/needs_review 只允许显式续跑。所有 Vault 查询带 VaultContext。

后台执行不使用普通 memory job handler。由 server composition root 启动一个有界初始化 supervisor，接收持久化任务 id；worker 先获取共享 Offline lease，再调用 application service/Core controlled permit，完成恢复、manifest、retire、索引删除、checkpoint、final scan 和 finish。任务在 lease drain 失败时持久化失败，不删除任何文件。

### Admin API

在当前 Vault 路由下增加：

- `GET /memory/initialization`：返回 phase、preview counts、manifest progress、任务状态和安全错误。
- `POST /memory/initialization/preview`：只读刷新预览。
- `POST /memory/initialization/start`：要求 CSRF、明确 `confirm_discard_legacy_memory=true`，创建或返回任务并立即 `202`。
- `POST /memory/initialization/resume`：仅对可续跑 failed/clearing 任务有效，要求再次明确确认。
- `GET /memory/generation` 保持兼容，并合并 initialization status。

Handler 只做鉴权、Vault scope、确认和任务调度；文件、SQL、Core 和 Provider 工作留在 MemoryService/worker。

### 启动与恢复

启动只检测 required/clearing 并将状态暴露给 Admin，不自动清理。任务进程中断后状态保持 clearing/failed，页面显示“需要续跑”；恢复任务复用已有 manifest.completed_files，不重新清理已完成文件。显式确认后先持久化 manifest，再将 source/destination/Core payload path 全在 managed legacy `_mcp-vault/memory/` 的未决 journal 直接终结为 `discarded`，之后仍通过 Vault Core 按 manifest hash 删除文件；不对旧 memory intent 调用 recover 或 replayed-create repair。普通 Vault 路径 journal 保留给常规 Core recovery，不改动也不阻塞此清理。无法证明纯 legacy 范围或跨出该 namespace 的 journal fail closed。

## 工作分解

1. 增加/更新维护 lease 和共享互斥：`crates/domain/src/maintenance.rs`、`crates/backup/src/lib.rs`、server composition root、Admin middleware；补充 drain timeout/竞态测试。
2. 扩展 memory initialization state/repository 和 migrations：`crates/state/src/units/initialization.rs`、`migrations/0030_memory_initialization_tasks.sql`、`migrations/0031_memory_initialization_diagnostics.sql`；实现 Vault-scoped preview/task transitions与安全阶段诊断。
3. 抽取现有 `crates/memory/src/v3/initialization.rs` 为可由 CLI 与后台任务共享的 service，保留 `StateStore::is_offline_exclusive` 保护并增加受控 permit。
4. 增加 server initialization supervisor 与重启恢复扫描；禁止普通 workers 抢占控制任务。
5. 增加 Admin API DTO、路由、认证/CSRF/Origin/隔离测试。
6. 增加 Admin UI 预览、确认、进度、失败与续跑交互，进行实际渲染检查。
7. 更新 ADR/README/运维文档和 fixture，执行 Rust、前端和临时端到端验证。

## 进度

- [x] 完成方案与风险审查。
- [x] 创建本 ExecPlan。
- [x] 更新 ADR-0033 的在线受控维护入口决策。
- [x] 实现共享维护 lease、独占 owner、有限 drain 和受控 permit。
- [x] 实现持久化任务与 Memory application supervisor；普通 job handler 不领取控制任务；服务 shutdown 会 abort 未完成句柄并保留 durable task，重启后由持久 task 状态和显式 resume 重新派发。
- [x] 实现 Admin API 和认证、Origin/CSRF、Vault scope、确认/202/失败续跑入口。
- [x] 实现 UI 确认、轮询、进度、失败与续跑交互。
- [ ] 完成临时 fixture、真实后台成功/失败续跑与 Backup 竞争的独立集成证据，以及全量测试与文档收尾。

## 决策

- Admin 确认后立即返回 202；任务由独立初始化 supervisor 执行，避免 HTTP handler 和普通 worker 持有会阻塞自身 drain 的 guard。
- 初始化、备份、恢复共享同一个 Offline lease；仅排空前失败恢复原模式，已开始数据变更的失败保持 Offline，不自动删除。BackupService 的创建、恢复和维护恢复都经过相同 owner/drain 路径。
- manifest/checkpoint 继续作为文件级幂等边界；任务状态仅描述调度和操作进度，不复制文件清单。
- 保留 CLI 作为受控离线运维入口，新增 Admin 入口复用 application service；CLI 的 offline exclusive 检查不削弱。

## 风险与缓解

- 维护切换竞态：统一 lease 先切 Offline 再等待 admission drain，超时回滚。
- 重启重复清理：事务状态和 manifest checkpoint 先于每个文件进度提交，ready/clearing/failed 明确区分。
- Vault 越权：所有路由从当前认证上下文派生 Vault，服务层再次校验 VaultContext。
- 旧 journal 被错误重放/跨界静默删除：仅显式初始化路径可 discard，并在 manifest 持久化后先校验 journal 完整 path set；跨界/未知范围失败且保留行。普通/Core global recovery 不变。
- Offline 锁死 Admin：middleware 只放行认证、会话、必要 Vault 列表和初始化/恢复状态路由。
- 新 v3 数据误删：旧文件 namespace allow-list 与 legacy 表白名单保持不变，ready 分支不执行清理。

## 验证

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `pnpm --dir frontend/admin lint`
- `pnpm --dir frontend/admin test`
- `pnpm --dir frontend/admin build`
- 临时 fixture：预览 -> 确认 -> 202 -> 后台进度 -> ready；失败 -> 重启展示 -> 显式续跑；跨 Vault、普通写入 drain、Backup 竞争、ready 幂等。
- 失败时保留精确命令和输出，不运行真实 Provider 或生产操作。

已完成的局部证据：

- `cargo check -p mcp-vault-domain -p mcp-vault-backup -p mcp-vault-server -p mcp-vault-admin-api` 通过。
- `cargo fmt --all --check` 通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `cargo test --workspace --all-features` 通过；迁移版本断言已更新为当前 migration version 33。
- lease ownership、state task 并发确认、journal 聚合、memory initialization 和 Backup archive 回归通过。
- Admin API 初始化状态/确认/202 测试通过；Admin API 共 27 项测试通过。
- server startup interruption marker、state resumable marker、Backup 三项回归测试通过；startup 在其它 Vault 扫描完成后才保持 Offline。
- canonical `ready` 与 stale task 的 metadata-only reconcile 路径已实现；不会重新清理文件。

前端验证已由独立代理完成：eslint、Vitest 3 个文件共 39 项、TypeScript `--noEmit`、Vite build 和 diffcheck 均通过；尚未进行浏览器截图或真实生产容器验收。

独立 service fixture 已通过后台成功、失败后 Offline + 显式 resume、已有 operation drain、新 Vault ready 四项；尚未完成真实生产部署验收。

旧记忆 journal 的后续修正单独记录于 `legacy-memory-scoped-journal-recovery.md`：确认 discard 后直接终结受管 legacy memory namespace 内纯 legacy journals，再走 hash-guarded Core retirement；普通笔记和跨界 journal 保持不变。该修正的 State/Core/Memory 定向 tests、Clippy、migration release check 和格式检查均通过。

## 回滚与恢复

迁移只新增任务元数据列/表和 `operation_journal.discarded` 终态；该 schema 迁移前向不可直接回滚。后台任务失败保持 manifest 和安全诊断；跨界/未知 journal 由运维处理后再显式续跑。服务重启跳过 pending Vault 的 recovery/initial scan，仅展示 clearing/failed 任务，不自动触发清理。

## 结果

Admin 一键初始化、持久任务、共享维护 lease、重启中断标记、失败续跑、ready 元数据修复和 UI 交互已实现并通过 workspace Rust gates、Admin/service 集成测试和前端静态/DOM 构建检查。浏览器截图、真实部署容器和生产数据验收仍需在目标环境执行。
