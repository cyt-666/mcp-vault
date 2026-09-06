# MCP 全工具说明与精简返回验收

状态：完成。代码、参数/说明审查、全工作区和官方协议回归通过。

## 逐工具审查与落地

所有 17 个工具均检查了选择条件、参数含义、默认值、返回字段和后续操作，
并更新 description。统一 include_details 默认 false，获取扩展元数据；get_memory
仍默认完整。鉴权、写前置条件、幂等键和删除副作用保持原业务规则。

| 工具 | 默认返回/参数修复 | 明确的下一步 |
|---|---|---|
| vault_overview | 保留统计/主题/覆盖，移除内部 Vault ID 和重复版本；limit 是主题/最近记录各自上限 | topic id → browse_index；已知路径直接读 |
| browse_index | 移除节点排序/来源内部字段与候选链接图；修复 depth=0 不展开子节点 | child id → browse_index；candidate.path → read_note；cursor 翻页 |
| recent_changes | 保留操作、前后路径、修订号、时间，移除内部 actor/行 ID/hash | 当前 path_after → read_note；删除 path_before → note_history |
| search_notes | 默认省略完整 headings/links/tags 图谱和评分，保留命中摘要、位置、覆盖、降级及分页 | result.path → read_note；不重复搜索已知路径 |
| read_note | 增加实际 file_id；修复历史版本 size/hash 错用当前记录；隐藏未支持 selection 枚举 | 当前 revision → 写入前置条件；truncated 时增加 max_bytes |
| recall | 默认来源路径；省略内部集合 ID、canonical_path、统计和评分策略；移除过时评测门槛说明 | sources[].path → read_note；缺来源再 get_memory，必要时搜索 |
| get_memory | 保留完整单条记录；明确 canonical_path 是托管记忆文件，canonical_revision 不是记忆写入版本 | sources[].path → read_note；revision → update/forget |
| list_memories | 精简记录，保留内容、ID、修订、来源及有值的置信度/有效期 | path → read_note；id → get_memory；cursor 翻页 |
| remember | 精简保存回执中的 memory，保留 outcome 和修订；不强迫无来源的显式记忆虚构来源 | id → get/update；普通笔记变化走 note 工具 |
| update_memory | 精简更新记录；明确遗漏保留、null 清除、[] 清空，区分三种修订号 | 冲突重新 get_memory；源笔记变化走 edit_note |
| forget_memory | 保留小型删除回执及来源暂停副作用；明确不删除源笔记/不提供撤销 | 冲突重新 get_memory；来源恢复在 Admin 明确操作 |
| create_note | 保留 path/file_id/revision/active、操作回执/ETag，省略内部数据库身份 | 已存在则读后决定；成功后直接复用路径 |
| edit_note | 同上；明确最小编辑、唯一标题和冲突处理 | 后续授权编辑用新 revision；冲突重新读 |
| move_note | 同上；明确成功后用新路径 | new path → read/write；冲突重新读 |
| delete_note | 保留 inactive/删除操作；隐藏不支持的 permanent 枚举 | history → 历史读取；恢复须明确授权 |
| note_history | 新增 limit/cursor，默认 25、最多 100，最新在前；不宣称返回 diff | 选择 revision → read_note；next_cursor 翻页 |
| restore_note_revision | 区分目标历史版本和当前/墓碑前置条件 | 新路径直接读；冲突刷新状态 |

## 协议和上下文取舍

`include_score_breakdown:true` 保留扩展诊断模式；Admin 和应用服务 DTO 不变。
默认来源在 memory 服务预算计算前启用，不能在预算检查后无界追加路径。
get_memory 和精确 read_note 是有意保留的详细读取入口。Compact sources 只含 path；
详细模式保留 file_id、revision、heading 和行号。相关笔记不等于某条记忆的来源。

RMCP 3.0.1 的 CallToolResult::structured 自动生成 text 与 structuredContent
两种兼容表示。本次保留 SDK 行为，仅缩小共享数据，不附加第三份解释文本。
公开 HTTP 样本中 recall data 从 911 字节降到 262 字节（约 71%），保留同一条
记忆和来源路径；仅为该样本的序列化字节测量，不是全库/所有模型的 token 保证。

## 验证记录

- MCP 测试 25 项通过，含所有工具的 OAuth 真实路由、权限/跨 Vault、当前删除语义，
  新增默认来源直接读取、来源显式关闭、list 来源、详细输出、SDK两种内容一致、
  历史内容元数据和分页顺序。旧完整字段断言改为显式 include_details。
- 初次编译发现历史 size 为 Option；初次回归中的四处旧扩展字段断言仍按旧默认值，
  以及新增测试缺少 Value 类型路径；修正并保留失败日志，无放宽权限/业务断言。
- `cargo test --offline --locked -p mcp-vault-mcp -- --nocapture`：25 passed。
- 工作区全特性 Clippy 通过；全特性测试 327 passed，0 failed。官方 2026-07-28、2025-11-25、2025-06-18、2025-03-26 四版本均通过已有 baseline；未新增例外（2026 的未实现 prompts 缓存场景仍为既有预期失败）。
- 未调用真实模型，未读生产 Vault。工具契约改进不能保证所有 Agent 遵从；Host 需刷新工具发现缓存。

## 升级 / 回滚

无数据库迁移；重建服务后重连 MCP 客户端刷新 tools/list。需要旧扩展字段的调用者
传 include_details:true，原字段保持可获取。回滚匹配旧二进制即可，无数据恢复步骤。

## 实际最终命令

- `ORT_LIB_LOCATION=/home/cheng/code/mcp-vault/target/memory-review/followup-tools ORT_PREFER_DYNAMIC_LINK=1 LD_LIBRARY_PATH=/home/cheng/code/mcp-vault/target/memory-review/followup-tools cargo test --offline --locked --workspace --all-features`：exit 0，327 passed。
- `cargo clippy --offline --locked --workspace --all-targets --all-features -- -D warnings`：exit 0。
- `cargo build --offline --locked -p mcp-vault-server --bins`：exit 0。
- `CARGO_NET_OFFLINE=true npm_config_offline=true MCP_VAULT_CONFORMANCE_PACKAGE=file:/home/cheng/code/mcp-vault/target/memory-review/conformance-src/conformance-74edef34d674f563537be8c6587cebaa58e830ca MCP_VAULT_CONFORMANCE_SPEC_VERSION=<version> MCP_VAULT_CONFORMANCE_OUTPUT_DIR=<isolated-output> bash scripts/conformance/mcp.sh`：上述四版本 exit 0；完整日志 mcp-tools-conformance-<version>.log。
- `cargo fmt --all --check`、`bash scripts/check-docs.sh`、`git diff --check`：通过。

本轮没有前端/迁移/WebDAV修改，未重复前端或 Litmus 检查；未更新生产镜像、部署或提交代码。
