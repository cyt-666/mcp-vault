# 原文分组与去重提示词 v2 真实复验

状态：已完成执行与独立审查（C1–C3 未执行）  
负责人：Luna  创建日期：2026-09-10  
上次更新：2026-09-10

## 目的与边界

使用 `auto-v2/requests.json` 对新版用户提示词做一次有界真实 Provider 复验。最多执行原六案例各一次，共六次 Provider 尝试；沿用上轮 `mimo-v2.5` Provider 配置的只读复制、生产 `ProviderService`／Transport、零重试设置和请求预算。新结果写入独立的 `auto-v2/run-01`，不覆盖 auto-v1 或根级旧结果。

本轮只验证真实输出是否能通过结构校验并保存完整原文展示。任何 Provider、schema 或引用结构错误立即停止，不追加重跑；`status: ok` 只表示结构通过，不表示语义质量通过。语义审查由 Astra 独立完成。

## 冻结证据

- 请求文件：`target/memory-block-grouping-experiment-20260910/auto-v2/requests.json`
- schema：3；案例：R1、R2、R3、C1、C2、C3；每案最多一次。
- requests SHA-256：`e614a4d04e26bc839a1b0ab234663e4dd3f7d1f38e6b324b2a686f2bcc0ddacc`
- 提示词文件 SHA-256：`88d0c5a8c34db0d855c5ac1d801261e9af040a2d91621612b9e1f13725853752`
- runner prompt 拼接 SHA-256：`290520c43ef516a2a6acb94972386fa0af1a9c9d8932d2cb42e8e19cdd038ffb`
- 新旧六案 `user` 载荷：逐字相等；新版仅更换 system、schema 名称和 schema 版本。
- 新增重点：groups 数组展开后的 display 顺序，按 `(source_id, full_source_sha256)` 保持字节／行定位顺序；不同快照不比较。
- 旧内容质量标准沿用 `auto-v1/frozen/expected.md`，不修改该文件。重点包括 C2 排他性、R2 触发操作归属、R1/R3 独有公式变量与顺序、C1 冲突顺序、C3 跨项目范围和周日信息。

## 输入与配置

Provider 配置从上一轮隔离 run 的只读 state 复制：

- source state：`target/memory-block-grouping-experiment-20260910/auto-v1/run-01/state/mcp-vault.sqlite3`
- source master key：`target/memory-block-grouping-experiment-20260910/auto-v1/run-01/secrets/master-key`
- source manifest model：`mimo-v2.5`
- requests：`target/memory-block-grouping-experiment-20260910/auto-v2/requests.json`
- run root：`target/memory-block-grouping-experiment-20260910/auto-v2/run-01`

密钥只作为现有解密服务的输入，不写入报告、日志或模型输出摘要。

## 执行与停止规则

运行前验证 run root 不存在、请求哈希和提示词哈希匹配、六案 `user` 载荷与 auto-v1 相等。runner 设置 Provider `max_retries=0`，预算上限为 6 次 transport reserve，并逐案持久化原始结构化输出、结构化校验结果、输入块和机械渲染结果。

遇到 Provider error、返回 schema 无法解析、编号／分组／覆盖／顺序结构不合法时，保留已完成结果并停止；不得因失败消耗下一次尝试。正常完成后运行渲染脚本，不修改输出正文。

## 进度

- [x] 核对旧 run 配置、mimo-v2.5、请求哈希、提示词哈希和六案 user 载荷。
- [x] 写入本轮真实复验计划，冻结顺序要求与旧 expected 语义审查重点。
- [x] 执行六案真实调用；R1、R2 完成，R3 触发结构首错后停止，C1–C3 未调用。
- [x] 生成机械 display.md 并核对 run 状态、尝试数、usage、耗时和错误边界。
- [x] Astra 完成对 R1、R2、R3 raw proposal 的独立审查；C1–C3 因首错未执行，不能评价其语义效果。

## 验证与报告

- 预检：Python 哈希／载荷比较和 schema 3 检查。
- 运行：`target/debug/memory-block-grouping-experiment run ...`，只使用本计划的 source state、key、requests 和 run root。
- 结果：`auto-v2/run-01/run-manifest.json`、`grouping-results.json`、`experiment.exitcode`、`display.md`。
- 报告记录应用调用数、transport 尝试数、usage、每案耗时、最终状态和停止原因；usage 缺失时标为不可用。

## 恢复与后续

本轮只产生独立 run root；失败时保留检查点，不重跑、不覆盖。旧 auto-v1 结果和 expected 保持原样。真实输出通过结构校验也不能证明语义质量提升，后续判断由 Astra 根据实际完整原文、上下文和旧 expected 标准完成。

## 本轮结果

`auto-v2/run-01` 最终状态为 `stopped_on_first_error`，共 3 次 Provider 尝试。R1（93113 ms、8245 Tokens）和 R2（149187 ms、13769 Tokens）结构通过；R3（154393 ms、15968 Tokens）因 W1 的 `B034`（字节 6865）先于 `B028`（字节 5848）展示而被全局同源快照顺序校验拒绝。C1、C2、C3 未执行。完整结果、usage 和机械展示位于 `auto-v2/run-01/grouping-results.json`、`auto-v2/run-01/report.md` 和 `auto-v2/run-01/display.md`；进程退出码为 1。

Astra 的审查记录位于 `auto-v2/run-01/review.json`。R1 无去重收益；R2 接受三项覆盖但记录 `dependency_readability_not_established`，撤回 B020→B013；R3 除顺序失败外还有错误覆盖引用和不相容分组。C2 的“只有一个例外”在本轮未知。由于 system prompt 与结构契约同时变化，且只完成部分样本，本轮不宣称整体改善或新版语义通过。
