# Admin 当前记忆批量删除

- 状态：实现与本地工程验收完成，待主代理审查
- 负责人：Codex Luna 执行，主代理审查
- 创建日期：2026-09-14
- 最近更新：2026-09-14

## 目的与用户可见结果

Admin「记忆」页面可选择多条当前记忆、全选本次已加载记录、取消选择，并通过明确确认一次提交批量删除。服务按每项 `id` 和 `expected_revision` 返回删除、冲突或失败结果。显式记忆按既有规范文件删除路径处理；同一 note-derived source set 中选择的项目合并为一次整体改写。批量操作保留来源当前暂停状态，因本批次删除而新暂停来源的行为只存在于现有单条 DELETE。UI 和接口说明两种语义，并提示未暂停来源可通过现有「重新提取全部笔记」显式重新评估（会增加模型调用成本）。

本功能只按通用 memory ID、revision、ownership 和 source-set 元数据工作，不根据笔记类型、路径或正文作筛选、判断或改写。

## 约束依据

- [产品要求](../../product-requirements.md)：3.5 长期记忆的来源所有权、完整 source-set 发布与可删除当前记忆。
- [架构](../../architecture.md)：Admin handler 只鉴权、校验并调用应用服务；Vault Context 是所有用户数据操作的隔离边界。
- [接口](../../interfaces.md)：当前记忆 Admin API、认证会话、Origin 与 CSRF 保护。
- [记忆系统](../../memory-system.md)：规范 Markdown、Vault Core 写入、note-derived 完整集合与来源暂停语义。
- [管理与配置](../../admin-and-configuration.md)：Memory 页面行为。
- [ADR-0033](../../adr/0033-source-preserving-memory-units.md)：当前 source-set/explicit 所有权及既有单条删除语义。
- [AGENTS.md](../../../AGENTS.md)：Vault Core 写入、Vault-scoped 状态、敏感正文不进日志与审计。

## 当前仓库状态

Admin `DELETE /memories/{id}?expected_revision=...` 调用 `MemoryService::forget`。显式记忆删除规范文件；note-derived 删除通过 Vault Core 重写拥有的 source set，并设置 `extraction_paused=true`。Admin 当前记忆列表分页加载，每页 50 条，提供逐条删除。`POST /memory/extraction/run` 已支持 `include_evaluated=true`，允许操作员显式重新处理未变化来源。

State 的 `memory_unit_snapshots`、`prepare_note_set_snapshot`、`publish_note_set` 及 MemoryService 的 prepared-set 恢复实现可用于保留规范文件、revision guard 和崩溃恢复边界。工作树可能含其他用户改动；本计划只涉及列出的相关文件。

## 范围与非范围

包含：Vault-scoped、认证且 Origin/CSRF 受保护的批量 Admin endpoint；严格的 ID/revision 输入验证；显式与派生记忆混合结果；note-derived 同源项目合并为一次 set rewrite；每项安全结果；Admin 多选、当前已加载项全选/取消和确认/结果反馈；API/UI/记忆契约文档；服务、API、授权边界和 UI 回归。

不包含：修改单条 DELETE；读取、分类、过滤或改写任何具体笔记内容；按笔记类型、路径或正文选择项目；MCP 批量删除；自动调用模型；跨请求全局事务；生产部署/验收。

## 不变量与风险

- 所有每项读取、向量删除和投影删除都绑定当前 `VaultContext`；跨 Vault ID 对外表现为通用不可用失败，不泄露其他 Vault 记录。
- 每项必须携带正数 expected revision；未知字段、空批次、重复 ID、无效 ID/revision、超过上限的请求在任何写入前整体拒绝。
- 显式项分别使用规范 Markdown 与期望 canonical revision；冲突不得覆盖新版本。
- 一个 note-derived source set 的成功批次只准备/发布一次完整 set；保留未选中的当前项目与来源 provenance；CAS 使用 set revision 和源 File ID/hash/revision。
- 批量 note-derived 删除保留原 `extraction_paused` 状态。预先暂停的 source 仍暂停；本操作不会新暂停未暂停 source。失败/冲突按 source set 粒度报告给该组有效选择项。
- 成功结果不含正文、路径、元数据或底层异常；审计只记录数量/结果分类，不记录记忆正文。
- 文件提交和 projection 发布沿用已持久化 prepared snapshot 与 Vault Core；崩溃后遵从现有恢复流程。

## 设计

新增 `POST /memories/bulk-delete`，body 为 `{items:[{id, expected_revision}]}`，最多 100 项且必须非空、ID 唯一。响应含每个请求 ID 的 `deleted|conflict|failed`、可选 ownership、已知暂停状态和固定安全错误码，以及计数汇总。HTTP handler 解析/验证请求、派生当前 Vault 和 Core、调用 `MemoryService`，并写入不含正文的批量审计事件。

应用服务一次取得 Vault 写锁，Vault-scoped 加载当前与 unchecked 投影，先按 item revision 过滤，再分别处理 explicit 和按 note set 分组的 note-derived 项。每组只从当前完整 set 构造一次剩余项 snapshot，保留 existing pause flag，并使用现有 prepared snapshot 与 Core publish 链路提交一次。显式记录独立处理，因此混合批次允许部分成功；同一 source group 内发布是整体的。最终结果按输入顺序返回。任何准备/发布失败都返回固定分类，不暴露底层错误。

UI 只对已加载页面行提供选择；全选/清除选择不触发额外读取。批量确认显示选中数以及 explicit/note-derived 数量，不显示或引用笔记/记忆正文。确认后发一个 POST，不循环调用单条 DELETE。部分结果保留成功项、清除已删除的选择并提示冲突项刷新；完整结果面板不展示敏感正文。单条按钮和 endpoint 继续原语义。

## 工作分解

1. 在 `crates/memory/src/v3/model.rs` 定义受限 batch 输入/结果类型，在 `service.rs` 实现 Vault-scoped 批处理与同源一次性 set rewrite。验证 memory v3 service 集成测试。
2. 在 `crates/admin-api/src/lib.rs` 增加 endpoint、严格 DTO、CSRF/Origin 检查、安全结果/审计及 request/auth/Vault 隔离测试。
3. 在 `frontend/admin/src/pages.tsx` 实现选择/确认/反馈，并在 `App.test.tsx` 或专门测试覆盖混合结果、选择范围、取消选择、单请求和冲突。
4. 更新 `docs/interfaces.md`、`docs/admin-and-configuration.md`、`docs/memory-system.md`，保留单条差异；完成 ExecPlan 的验证与剩余限制。

## 进度

- [x] 2026-09-14：核对 e046ceb 提交后的接口、服务、Admin 页面、分页和既有单条删除语义。
- [x] 2026-09-14：确定批量同源单次改写，保留原暂停状态；显式项按项处理，source group 整组 CAS。
- [x] 实现服务/API/UI 与回归测试。
- [x] 更新公开接口、管理和记忆规范。
- [x] 运行相关 Rust/前端验证与格式/静态检查。

## 决策

- 单条删除仍通过当前 endpoint，并保持 note-derived 删除后暂停来源的原行为。
- 批量删除明确代表对选择项作局部删除：同源合并一次完整 source-set rewrite，不因批量命令暂停新来源。原已暂停的集合不自动恢复。
- 输入级无效（包含重复/超限）在写入前拒绝整个请求；运行时项目级冲突/失败以结果项返回。不同 explicit 项与不同 source group 可以部分成功。
- 重新提取通过既有显式「重新提取全部笔记」操作支持；不在删除请求内调用模型或扩大功能到自动重提取。

## 意外发现

- Admin 批量更新使用 durable prepared source-set snapshot。当前暂停状态必须进入 snapshot 才能保留；State 对 active set 还要求原 Provider/model identity 完整。已有提取集合满足此契约，批量路径没有创建新的 extraction/model request。
- `pnpm --dir frontend/admin test -- App.test.tsx` 在沙箱内尝试从 `https://registry.npmmirror.com/pnpm` 获取 pnpm 并报 `[ERR_PNPM_META_FETCH_FAIL]`，随后因 `[ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY]` 退出。未清理 `node_modules`；直接运行已安装的 `frontend/admin/node_modules/.bin/` 工具完成了 lint、Vitest、TypeScript 与 build。

## 验证

- `cargo test -p mcp-vault-memory`：通过，23 unit、9 Admin initialization service、1 initialization fixture、11 v3 integration tests。
- `cargo test -p mcp-vault-admin-api`：通过，28 tests；新增 batch endpoint security/result test 通过。
- `cargo clippy -p mcp-vault-memory -p mcp-vault-admin-api --all-targets --all-features -- -D warnings`：通过。
- `cargo fmt --all --check`、`git diff --check`：通过。
- `frontend/admin/node_modules/.bin/eslint .`：通过；`vitest run`：通过 3 个文件、43 tests；`tsc --noEmit`：通过；`vite build`：通过。
- 必测：explicit 删除、note-derived 多项同源一次 set revision bump、混合 ownership/来源、expected-revision 冲突、Vault 隔离/invalid input、认证/Origin/CSRF、UI 全选已加载项/取消/单一批量请求/部分结果。
- 不调用真实 Provider；语义/生产验收不在本任务内。

## 回滚与恢复

无 schema migration。回滚应用/API/UI 与契约文字即可；不恢复已经由既有 Core revision/history 删除的记忆。prepared note-set snapshot 遵从现有恢复机制。单条 DELETE 路由与语义不变。

## 结果

完成了 Vault-scoped `POST /memories/bulk-delete`、按 source set 分组的单次 rewrite、保留既有 pause 状态、显式项删除、Admin 选择/确认/逐项结果、接口与生命周期文档，以及服务/API/UI 回归。

服务集成证明同一 source set 两个选中项只增加一次 set revision 和一次 canonical revision；active source 保持 active，单项 DELETE 仍会 pause，paused source 经 batch 后仍 paused；active source 可以通过既有显式 include-evaluated re-evaluation 恢复删除单元。混合 explicit/note-derived、多 Vault ID、冲突、批次无效输入、认证/Origin/CSRF 和响应/审计脱敏均有测试。没有调用真实 Provider，没有声称生产验收。

功能已就绪供主代理审查；工作树变更尚未提交或推送。
