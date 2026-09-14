# 原文分组与去重提示词 v2：mimo-v2.5-pro 复验

状态：已完成执行与独立审查（首错停止，效果部分不通过）  
负责人：Luna  创建日期：2026-09-10  
上次更新：2026-09-10

## 目的与边界

在不修改生产绑定和既有实验结果的前提下，使用现有配置中的
`mimo-v2.5-pro` 对 `auto-v2/requests.json` 做公平的六案例复验。runner 增加
可选 `--source-model-id <UUID>`，默认仍从 `memory_extraction` 绑定解析模型；
指定时只读源库中的 model/provider 记录，并把它们复制到隔离实验状态。

每个案例最多调用一次，预算上限为六次 Provider/Transport 尝试，Provider 设置
关闭重试；首个 Provider、schema 或引用结构错误立即停止，不追加试探或重跑。
不启动正式服务、不调用 embedding、不调用 ASR/TTS、不发布正式记忆。Luna 仅
负责执行，Provider 模型与执行代理身份分开记录；语义结果由 Astra 独立审查。

## 冻结输入与公平性

- 请求：`target/memory-block-grouping-experiment-20260910/auto-v2/requests.json`
- 六案及全部 `user.blocks` 与此前 auto-v2 相同。
- system prompt、schema 3、全局同源同快照展示顺序契约保持不变。
- 对照模型：上一轮 `mimo-v2.5`；替代模型：配置中的 `mimo-v2.5-pro`。
- 两个模型的有效 `capabilities` 和 `settings` 在运行前记录并比较；Provider
  timeout、输出预算和重试策略的差异写入运行报告。

## 源配置与隔离

源库只读使用本机实际配置 `data/state/mcp-vault.sqlite3` 和其现有安装主密钥；
不会更新源库绑定或 Provider 配置。运行器将指定 source model 及其所属 provider
复制到新的 `auto-v2/run-pro-02`，并在 manifest 保存 model/provider ID、类型、
revision 和不含秘密明文的配置哈希。

## 工作步骤

1. 只读确认 `mimo-v2.5-pro` 的 UUID、启用状态、Provider 类型、角色绑定和配置元数据。
2. 增加可选 source model ID 参数；保持默认 `memory_extraction` 解析和旧调用兼容。
3. 运行原型离线 fmt、clippy、test/check/build，并确认请求与六案 user 载荷未变。
4. 使用 `--source-model-id` 在新 run-root 执行最多六次真实请求；保存 manifest、raw 结果、退出码、报告和机械 display。
5. 读取结果并交付 Astra；不将结构 `ok` 或单次对照结果写成质量通过或整体改善。

## 进度

- [x] 找到启用的 `mimo-v2.5-pro`：model UUID `01a03d4c-12b4-7e30-b3a9-4065d1821696`，Provider 类型 `xiaomi_mimo`。
- [x] 新建本复验执行计划。
- [x] 完成 runner 参数实现及离线检查。
- [x] 尝试执行 `auto-v2/run-pro-01`；启动命令在 Provider 调用前因 `state database error` 停止。
- [x] 保存失败证据；无 Provider 请求，未产生可供 Astra 语义审查的 Pro 输出。
- [x] 对原始配置库做 SQLite backup，在独立副本 `auto-v2/source-pro-seed.sqlite3` 上使用项目 `connect_and_migrate`；seed migration 29，Pro 行启用且完整。
- [x] 启动 `run-pro-02`，最多六次真实请求并执行首错停止；R1/R2 完成，R3 `provider_schema_invalid`，C1–C3 未调用。
- [x] Astra 完成对 R1/R2 的语义审查并记录 R3 无 output、C1–C3 未执行；本轮不判定模型整体优劣。

## 验证与停止

验证 command 必须使用 `--source-model-id 01a03d4c-12b4-7e30-b3a9-4065d1821696`，并检查 manifest 的 `external_model_id` 确为 `mimo-v2.5-pro`。记录每案 usage、耗时、调用数和最终状态；首错后只保存已完成结果。任何源库/key 不可读时立即报告，不猜测替代路径。

## 回滚与限制

代码参数改动可回退；源库绑定、生产配置和旧 run-root 不变。启动失败证据位于
`auto-v2/run-pro-01/preflight-failure.md`，Provider 调用数和 transport 尝试数均为
0。源 DB migration 11 与 runner 当前 migration 29 不兼容；后续只对独立 backup
副本调用 `connect_and_migrate`，不改原库、不合成密钥、不猜测 key/endpoint。原始
源库 SHA 在 backup 前后均为 `71cb26c9e304ed73c4ee1bfbde0ecc76ddd8691f311cb73e6f461b123ad69f82`。
新的真实运行目录为 `auto-v2/run-pro-02`，不复用已失败的 `run-pro-01`。模型、prompt
和结构契约同时变化时，不能把差异归因于单一因素。

## 本轮执行与审查结果

`run-pro-02` manifest 确认 `external_model_id=mimo-v2.5-pro`，请求 SHA 与上一轮
相同。R1 用时 117746 ms、9123 Tokens，因 `B006→B002` 缺少具体
`backward`／`optimizer.step()` API 记录 `unique_API_loss`；R2 用时 92743 ms、11986
Tokens，与上一轮结构完全相同，记录局部指代衔接不足；R3 用时 137767 ms，Provider
返回 `provider_schema_invalid`，无可审查 model output。累计 3 次尝试、348256 ms，
报告 usage 仅计 R1/R2 的 21109 Tokens，R3 未知。C1–C3 未调用。结果位于
`auto-v2/run-pro-02/review.json`、`report.md`、`grouping-results.json` 和 `display.md`。
本轮仅为部分样本，不能宣称 Pro 或整体提示词改善，也不能判定模型整体能力优劣。
