# ADR-0037：结构化 Provider 生成支持有界 SSE 接收

- 状态：实现中
- 日期：2026-09-22

Provider 结构化生成的流式模式只在支持 OpenAI-compatible Chat SSE 的 adapter/preset 上显式启用。SSE 增量仅用于接收和聚合；reasoning delta 不进入最终 JSON 正文。必须收到成功 finish reason、完整 content、完整 JSON/schema 校验后才返回 `StructuredGenerationResult`，否则失败并禁止重放已开始的生成。

流式接收使用连接超时、首事件超时、chunk idle 超时和有界总时限；非流式调用保持现有 timeout 语义。embedding、rerank 和不支持 SSE 的 adapter 不受影响。usage 缺失时保持 unknown，不伪造数值。

## 运行时矩阵

九类 generation Provider 中，七类 OpenAI-compatible Chat（OpenAI-compatible、DeepSeek、MiMo、GLM、Kimi、Gemini、Qwen）使用 Chat SSE；OpenAI Responses 与 Anthropic Messages 使用各自 native SSE 事件。EmbeddingHttp 与 FastEmbedLocal 不进入 generation streaming。新 eval 配置使用 600 秒显式 bounded total；Provider 默认首事件和 chunk idle 为 120 秒，connect 仍为 5 秒。调用方显式 request timeout 小于该值时优先使用显式值，非流式调用保留原 timeout 语义。

usage 缺失保持 unknown，不填零。reasoning/token 增量不进入持久化内容；当前范围不增加 UI token 展示。
