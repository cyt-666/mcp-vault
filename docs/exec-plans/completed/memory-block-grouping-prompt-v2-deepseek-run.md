# 原文分组与去重提示词 v2：deepseek-flash 复验

状态：已完成执行与 Astra 独立审查  
负责人：Luna  创建日期：2026-09-10  
上次更新：2026-09-10

## 目标与边界

使用当前运行 Admin 实例实际配置中的 `deepseek-flash`，对同一份
`auto-v2/requests.json` 做模型对照。每个原六案例最多一次、总上限 6 次
Provider/Transport 尝试，Provider 零重试；首次 Provider、输出 schema 或引用结构
错误立即停止，不追加调用。Luna 负责执行，Astra 负责后续语义审查；不把执行代理
当作 Provider 模型。

不启动或迁移正式服务，不修改 Admin UI、正式角色绑定或源库，不调用 embedding、
ASR/TTS，不发布正式记忆。

## 冻结输入

- 请求：`target/memory-block-grouping-experiment-20260910/auto-v2/requests.json`
- schema：3；案例：R1、R2、R3、C1、C2、C3；全部 user blocks 与此前对照相同。
- requests SHA-256：`e614a4d04e26bc839a1b0ab234663e4dd3f7d1f38e6b324b2a686f2bcc0ddacc`
- prompt SHA-256：`290520c43ef516a2a6acb94972386fa0af1a9c9d8932d2cb42e8e19cdd038ffb`
- 输出预算与 timeout 沿用 auto-v2 runner；若实际 Provider 行为产生差异，仅记录不修改公平输入。

## 实际模型配置

当前进程 PID 68796 打开的 state 为
`target/memory-organization-validation/live-20260908/configuration-01/state/mcp-vault.sqlite3`，日志确认 Admin 为 `127.0.0.1:49727`。只读查询得到：

- model UUID：`01a08aa9-3200-7291-b040-c28577200c6f`
- external model ID：`deepseek-flash`
- Provider type：`deepseek`
- Provider/model：均启用；Provider revision 2，model revision 1。
- capabilities：embeddings/reranking/structured_output 均为 false，其他值为 null。
- model settings：OpenAI-compatible preset、structured-output mode、thinking mode 和 token-limit field 均为 `auto`，generation token limit 为 null。
- 当前实际绑定：`memory_extraction`、`note_summary`、`rerank`、`topic_enrichment`。

源库 migration 最高版本为 27，因此只对隔离 SQLite backup 使用项目
`connect_and_migrate` 迁移到当前 runner schema；原始 state 保持不变。密钥只作为
现有解密服务输入，不出现在日志或报告。

## 执行步骤

1. 对当前实际 state 做 SQLite `.backup` 到独立 seed，记录源库 SHA 前后未变。
2. 用 runner 的 `migrate-seed` 对 seed 调用 `StateStore::connect_and_migrate`，确认 deepseek model/provider 仍启用且模型 UUID一致。
3. 确认新 `auto-v2/run-deepseek-01` 不存在后，以 `--source-model-id 01a08aa9-3200-7291-b040-c28577200c6f` 执行真实复验。
4. 每案完成后保存持久化结果并报告；首错后停止。结束后生成机械 display 和执行报告，交 Astra 审查。

## 进度

- [x] 从当前运行实例的实际 state 读取 deepseek-flash 元数据和绑定。
- [x] 记录请求/prompt SHA 与 schema 3。
- [x] 完成隔离 backup、官方迁移和 seed 核对；原始 source state 保持未修改。
- [x] 执行 `run-deepseek-01`；六案六次尝试均完成结构校验。
- [x] 保存结果并由 Astra 完成六案独立语义审查；本计划不自行扩展语义结论。

## 回滚与限制

所有写入限于本实验 seed 和 run root；源库、Admin UI、正式绑定和旧实验结果不变。
若源库 backup、key 解密或迁移失败，保存具体安全错误并停止，不猜路径或接口。模型、
prompt 和结构契约保持与已有对照相同，单次部分样本不能证明模型整体优劣。

## 执行结果

`run-deepseek-01` manifest 确认 `external_model_id=deepseek-flash`。R1–C3 六案均为
`ok`，Provider/transport 尝试 6 次，累计 92045 Tokens（prompt 32197、completion
59848），总耗时 272373 ms，退出码 0。结果、报告和机械展示位于
`auto-v2/run-deepseek-01/grouping-results.json`、`report.md` 和 `display.md`。
语义审查由 Astra 根据完整原文、context 和冻结 expected 进行；本轮不宣称模型整体
优劣或提示词整体改善。

## Astra 审查结果

Astra 总体判定为 `six_case_content_and_coverage_passed`。六案共 60 个输入块、41 个
展示块、19 个省略块；R1 保留全部内容但无去重收益，R2/R3/C1/C2/C3 的覆盖、范围、
顺序和实际 context 均通过审查。展示正文为 2647 字符；按实际展示块重复计数 context
后的正文加 context 为 12197 字符。该口径不等于 Token 或净成本压缩，也不提供一般无
损保证、生产接入、全库入选或 recall 通过。

与同 prompt 的 MiMo/Pro 共同执行 R1–R3 比较，DeepSeek R1 保留 API 细节、R2 局部
依赖更好、R3 可用且覆盖正确；C1–C3 未在 MiMo/Pro 中同条件执行，因此不作这些案例
的胜出结论。旧 MiMo C2 失败不能单独归因于 prompt 或 model。详见
`auto-v2/run-deepseek-01/review.json` 和 `report.md`。
