# 原文分组与去重提示词 v2 契约适配

状态：已完成（仅实验原型，未调用真实 Provider）  
负责人：Luna  创建日期：2026-09-10  
上次更新：2026-09-10

## 目的与结果

在既有原文分组与去重实验旁建立本轮用户提示词的独立 schema 版本和请求生成入口。新版请求复用 auto-v1 已冻结的原文块与案例编号，保存归一化提示词，并让离线 runner 对空输入、`G1` 顺序、覆盖引用唯一性及跨组同源同快照展示顺序执行确定性结构校验。

本计划不进行语义判断、不声称语义质量提高、不调用真实模型、不启动服务、不接入正式记忆，也不修改长期记忆 selection 提示词。

## 依据

- [产品要求](../../product-requirements.md)：原文来源、当前记忆透明性和 Provider 失败隔离边界。
- [系统架构](../../architecture.md)：实验工具不绕过正式协议和 Provider 边界。
- [ADR-0033](../../adr/0033-source-preserving-memory-units.md)：完整原文、来源保全和去重不取代正文。
- [既有实验计划](../completed/memory-block-grouping-experiment.md)：auto-v1 和旧 run-01 作为不可覆盖的历史证据。

## 当前状态

实验 runner 位于 `target/memory-block-grouping-experiment-20260910`，原始 `frozen/`、根级 `requests.json`、旧 run-01 以及 `auto-v1/` 均已有运行证据。旧 validator 会在每组内检查源顺序并排序展示编号；这属于旧契约语义，必须继续可重现。

## 范围

包含：

- 新增 `auto-v2/frozen/system.txt`，逐字采用用户提示词并把 `\_` 恢复为 `_`。
- 新增 `auto-v2/prepare_requests.py`、请求和 README，显式生成 schema 版本 3，并复用 auto-v1 块及编号。
- runner 按请求 schema 版本分派旧／新结构校验；v3 保留模型 display 顺序，检查 `G1`、`G2`……、覆盖引用无重复、空输入和 `(source_id, full_source_sha256)` 全局顺序。
- 添加离线 Rust 与 Python 回归检查。

不包含：真实 Provider 运行、服务启动、生产发布、自动切块语义改进、长期记忆 selection、全库质量评估、git 提交。

## 不变量与风险

- 不覆盖任何既有冻结输入、请求、auto-v1 文件或 run-01 结果。
- 版本 1、2 的 runner 校验行为保留；不能根据提示词文本猜契约版本。
- v3 只做可机械证明的结构与定位顺序检查，不把它当作语义覆盖证明。
- 不同文件快照不比较顺序；同一 `(source_id, full_source_sha256)` 的展示块按 groups 展开顺序比较字节／行定位。

## 工作步骤

1. 读取规范、ADR-0033 和既有实验计划，确认实验边界。
2. 保存用户提示词归一化版本并建立 auto-v2 请求生成器，验证请求仍使用冻结块和既有编号。
3. 将 runner 请求 schema 分为 1、2、3，保留旧 validator；新增 v3 validator 和输出版本透传。
4. 为空输入、漏组、G1 序号、覆盖引用重复、跨组顺序和不同快照补充离线回归。
5. 运行格式、check、test、build 及请求／提示词一致性检查，记录证据和局限。

## 进度

- [x] 读取项目要求、架构、ADR-0033 和既有实验计划。
- [x] 新增 auto-v2 提示词、生成器、README，并生成 schema-v3 请求。
- [x] 按显式 schema 版本分派 runner 校验；旧版本保留旧排序语义。
- [x] 完成离线结构回归、格式、check 和 build。
- [x] 主 Agent 审查改动；修正重复覆盖引用回归并完成收尾。

## 验证

- `cargo fmt --manifest-path target/memory-block-grouping-experiment-20260910/Cargo.toml -- --check`
- `cargo test --offline --manifest-path target/memory-block-grouping-experiment-20260910/Cargo.toml`
- `cargo check --offline --manifest-path target/memory-block-grouping-experiment-20260910/Cargo.toml`
- `cargo build --offline --manifest-path target/memory-block-grouping-experiment-20260910/Cargo.toml`
- `python3 auto-v2/test_prepare_requests.py` 与生成请求字段检查。
- 提示词逐字归一化检查：逐段对照用户原文，确认仅恢复 Markdown 下划线转义；文件 SHA-256 记录在交付报告中。

## 回滚

本实验只新增 auto-v2 文件并修改实验 runner 源码。删除或回退这些新增／适配文件即可恢复原实验；旧 frozen、auto-v1 和 run-01 无需迁移。未创建 Provider run-root，不存在真实调用恢复问题。

## 结果与后续

离线结构验证通过后，仍只能说明请求契约和机械引用闭合可执行；不能说明模型能正确分组、覆盖或提升语义质量。旧 auto-v1 请求 SHA 为 `689b4bdf5e4feeeef94c95de5e4b438dca22ac284efdff7fa406bbdd985b8398`，旧 expected SHA 为 `6c898cfd67925544c459c1eff9e5492272092cde77cfc65dea1b67559cacbad0`，新旧六案 `user` 载荷相等。后续真实复验需由单独授权的实验计划冻结输入、预算、结果和语义审查。
