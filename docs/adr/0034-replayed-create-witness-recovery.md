# ADR-0034：有证据的重放 create journal 终结

- 状态：已接受，实施中。
- 日期：2026-09-11。
- 相关执行计划：[重放 create journal 恢复](../exec-plans/active/replayed-create-witness-recovery.md)。

## 背景

恢复一个 `file_committed` 的 `create` journal 时，目标路径可能已经被另一个 File ID 的活动条目占用。直接重试会再次触发 `require_absent` 冲突，直接把 journal 标记为已提交又会伪造原操作的 revision、audit 和 outbox 事实。生产诊断已经出现过这种形态：旧 journal 没有自己的 `file_entries` 或 `file_revisions`，后来的活动 File ID 具有相同路径、revision 1、相同内容哈希，并且有后来的 `metadata_committed` create journal。

## 决定

增加 `superseded` 终态。它只表示原始 create 的物理结果已经被后续、同路径、同内容的独立 create 元数据事实安全接管；它不表示原始 journal 已经完成，也不改变后来的 File ID、revision、history、outbox 或 canonical 文件。

State 在一个 `BEGIN IMMEDIATE` 事务内重新读取并校验全部证据，然后以 `state = 'file_committed'` 作为 CAS 条件更新原 journal，并写入一条 Vault 作用域 audit。校验必须同时满足：原 journal 属于当前 Vault、操作为 create、状态为 `file_committed`、payload 中 `require_absent` 为 true，路径和原 File ID 与调用参数完全一致；原 File ID 在当前条目和 revision 中都不存在；目标路径恰好由不同的活动 File ID 占用，活动 revision 和 create revision 的路径、revision、哈希一致；存在更晚的 `metadata_committed` create journal 证明该 File ID 的元数据提交；原 proposed hash、当前条目哈希和调用方重新计算的物理哈希完全一致；source path 只能为空或等于目标路径，不能出现 move 语义；temporary path 即使仍记录在 journal 中，也必须已经由 Core 证明物理文件不存在。

恢复错误还携带 journal 的 operation ID 和 Vault 相对路径，供初始化任务安全持久化阶段诊断；原有不带上下文的 Core recovery API 保持兼容。

任何一个条件不满足都返回未证明，调用方保留 `needs_review`。普通恢复只处理仍为 `file_committed` 的记录；显式维护恢复可以对错误标记恰为“目标已存在”的 reviewed create 重新执行同一物理验证，成功后才允许进入 `superseded`。旧记忆一次性初始化是用户明确授权抛弃整个 predecessor memory namespace 的专用流程；它不调用本 ADR 的 replayed-create repair，而是在严格证明 journal 的所有 canonical 路径都位于该 legacy namespace 后，将旧 intent 标为 `discarded`，再按独立 manifest/hash 通过 Core 删除文件。其它 Vault 路径或无法证明范围的 journal 不会被该流程修改。重复调用对已 `superseded` 的同一 journal 返回幂等结果，不新增 audit。普通 supersede 路径不删除临时文件，不修改后来的条目和历史，也不接受跨 Vault 的 ID 或路径作为证据。

## 后果

这能安全终结已知的恢复重放冲突，同时保留无法证明的情况供维护审查。`operation_journal` 的约束通过前向迁移扩展；历史迁移文件保持不变。恢复报告单独统计 `superseded`，避免把接管事实误报成原操作 metadata commit。

## 验收

- 精确 witness 通过，并留下单条审计记录。
- 哈希不一致、缺少后续 metadata journal、原 File ID 有历史、跨 Vault、replacement 已变化或存在临时路径时均拒绝。
- 重复恢复幂等，服务重启和迁移后状态保持。
- 第二个仍为 `prepared` 且无当前条目的 create journal 继续走普通恢复路径。
