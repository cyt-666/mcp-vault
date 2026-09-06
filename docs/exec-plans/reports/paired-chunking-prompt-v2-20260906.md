# 分块提示词修订与真实复测：2026-09-06

状态：已完成。此次复测未证明检索改善；模型采用率低，且一题正确笔记从第一降到第二。

## 修改与固定条件

用户要求明确分块的检索目的后重新测试。生产 `chunk_plan.rs` 提示词现明确：
每组独立生成 embedding；把同一事项的相关原文及适用条件、权限、否定、例外和限制
尽量放在一起，使命中块提供完整答案；避免仅因段落、小标题而碎片化，也不合并无关内容。
连续编号、完整覆盖、原文不改写与 UTF-8 大小约束仍然保留。

只修改提示词；模型、维度、大小限制、超时、排序和标签不变。使用上轮已查看的
`chunking-paired.json` 做开发集前后对照，不能称为新样本独立验收。
历史输出保留；本轮输出在 `target/memory-review/paired-prompt-v2-20260906/`，
包含实际提示词、冻结样本、请求账本及 prompt/fixture/binary SHA-256。
最多 24 次实际请求，只读隔离本地模型配置、处理临时合成 Vault。
原有已缓存计划不因提示词修改自动失效，不触发用户笔记重新生成。

## 命令与离线验证

- `cargo build --offline --locked -p mcp-vault-indexer --example paired_chunking_eval`：通过。
- `cargo test --offline --locked -p mcp-vault-indexer chunk_plan`：沙箱禁止回环监听，1 通过、1 失败；保留 `paired-prompt-v2-tests.log`。
- 允许本地监听后 `cargo test --offline --locked -p mcp-vault-indexer`：16 通过、0 失败。
- `cargo clippy --offline --locked -p mcp-vault-indexer --all-targets --all-features -- -D warnings`：通过。
- `cargo fmt --all --check`、`sh scripts/check-docs.sh`、`git diff --check`：通过。
- `target/debug/examples/paired_chunking_eval --source-data target/memory-review/local-model.09VRlm/data --output target/memory-review/paired-prompt-v2-20260906`：exit 0，11 次实际请求（8 次 LLM + 3 批 embedding），41 个唯一输入，2048 维。

本次聚焦提示词，没有修改前端、协议、迁移或其测试；未重复全工作区及前端验收。
不 push、不部署、不修改生产 Vault；本地页面运行中的旧服务未替换，测试使用新编译评测程序。

## 实测结果

| 指标 | 本轮规则组 | 旧提示词模型流程（上轮） | 新提示词模型流程 |
|---|---:|---:|---:|
| 采用模型分组 | 0/8 | 2/8 | 1/8 |
| 回退规则 | 不适用 | 6/8 | 7/8 |
| 分块数量 | 16 | 23 | 19 |
| 正确笔记第一名 | 16/16 | 16/16 | 15/16 |
| 首选块完整答案覆盖 | 16/16 | 14/16 | 15/16 |
| Recall@5 | 1.0 | 1.0 | 1.0 |
| MRR@5 | 1.0 | 1.0 | 0.96875 |
| 无答案题有非负相似度候选 | 4/4 | 4/4 | 4/4 |

本轮仅 `pine-irrigation` 采用模型分组（5 块），其余七篇均回退。
问题 q08 问节水结果是否适用于所有苗床，最高分却是 `pine-lab` 水质实验
（0.4119003）；包含“仅东侧二号苗床……不适用于其他苗床”的正确块排第二
（0.3969153）。答案没有被删除，但向量排序错配了主题相近的实验笔记。

不能把答案覆盖 14→15 解释为提示词已改善：两轮实际采用模型分组的笔记不同，
上轮退步的两篇本轮已回退，生成/服务响应和向量请求也不是固定输出回放。
当前回退仅存 rules，没有具体原因，7 篇为何回退仍不能精确归因。
此轮按授权只修改提示词，未修订回退诊断；后续应先增加不泄露原文的失败类别，
再定位分组未采用原因，而不是继续盲目收费重跑。

已保留实际 prompt.txt、provenance.json、requests.jsonl、report.json、两组
chunks.json 和 vectors.json；可离线复核，不需重新调用模型。
