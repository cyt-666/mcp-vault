# Admin 模型绑定保存失败待办

状态：Admin 应用层 role capability validation 已实现；定向后端测试通过；原始 UI 报告未在实际 Admin 环境复现，Zhipu 尚未配置或调用 Provider
创建日期：2026-09-10

## 用户报告

用户在当前 Admin 实例的“高级：摘要、Embedding 与重排模型”区域，为“记忆概览（可选）”选择 `deepseek-flash — deepseek` 后，点击保存无法成功。

用户提供的截图路径：
`/var/folders/v8/23q0r3m167x3qmk4rjtkyrn40000gn/T/codex-clipboard-d9eaa09d-87f1-41de-92e8-477176356c57.png`

## 原始报告与证据边界

- 原始 UI 报告仍未在实际 Admin 实例复现。
- 根因未知，不能据截图推断是前端校验、请求契约、角色绑定、Provider/model 状态或服务端错误。
- 本地 capability validation 和测试不证明截图中的故障已在现场修复；没有写入正式角色绑定、添加 Provider 或发起真实 Provider 请求。

## 后续入口

服务端绑定入口现已按角色要求模型 capability，并在绑定写入前拒绝缺失/禁用模型及 capability 不匹配。该改动只证明本地 Admin API 行为，不追溯截图中的失败原因，也未在实际 Admin 实例验证。

用户后续的向量模型配置方向为 Zhipu GLM：选择 `zhipu_glm`（控制台预填项目默认官方 API root），手动登记 `embedding-3`，声明 `embeddings=true`、dimension `2048`，绑定 `embedding_memory`，需要普通笔记语义搜索时再绑定 `embedding_note`。目前未收到 API key，也未获准实际写入配置或发起 Provider 请求；此路径仍待显式配置与独立运行验证。

### 本地实现与验证（2026-09-21）

- ProviderService 的 `bind_model` application boundary 现在确认候选 model/provider 存在且 enabled，再按 role 要求验证显式 capability，最后才执行带 `expected_revision` 的 State upsert。Admin API 将 capability 不匹配返回为稳定 `422 model_capability_mismatch`，不会写绑定、audit 或 embedding jobs；`memory_overview` 已加入 Admin binding role 列表。
- 定向 Admin API binding tests、Provider crate tests、Indexer tests 与 Memory tests 通过。以上使用隔离临时 State/local fakes；未触网、调用真实 Provider 或修改当前安装配置。
- Server 的 full-vault extraction regression 通过；Admin 前端本地 ESLint、Vitest（40 passed、10 skipped）、TypeScript 和 Vite build 通过。标准 `pnpm` lint wrapper 因尝试获取 npm mirror 上的 pnpm metadata 且非交互环境无法清理 modules 而退出，之后使用已安装本地二进制完成等价检查；未发出 Provider 请求。
- 未改变原始报告的状态：没有实际 Admin UI/服务端现场复现，也没有 Zhipu key 或 live capability 验收。

### 2026-09-22 当前 MiMo 绑定保存现场

只读 State 核验确认默认 Vault 的 `memory_extraction` 仍绑定
`mimo-v2.5`（revision 1）；`mimo-v2.6-flash` 已登记且启用，但其 capability JSON
明确为 `structured_output=false`，没有绑定到任何 role。ProviderService 的真实绑定门禁
要求 `memory_extraction` 具备 `structured_output`，因此选择该模型后后端会返回 422
`model_capability_mismatch`，不会写入 binding。该 capability 未声明不能由 UI 或服务端
擅自改成 true。

现场 UI 反馈不足：绑定控件原本只在异步请求失败后通过通用通知显示错误。修复后，
Embedding/reranking 仍在缺少真实操作 capability 时显示原因并禁用无效保存；生成类 role
对 `structured_output=false`/缺失只显示“将使用适配器兼容模式和本地 schema 校验”的信息，
不再把原生 JSON Schema capability 当成生成操作可用性的硬门禁。后端生成类 role 按 adapter
是否提供 `generate_structured` 操作校验，OpenAI-compatible/MiMo/Anthropic 允许，
EmbeddingHttp/FastEmbedLocal 拒绝；schema validation 仍在实际响应路径执行。新增 Admin 回归
覆盖未知原生 capability 的生成绑定成功与 readback，既有 embedding/rerank 拒绝回归保留。

验证：TypeScript、ESLint、`App.test.tsx`（18 passed、10 skipped）和 Vite build 通过；
重新构建并重启本地服务后，Admin bundle 为 `index-CL3H9E7h.js`，健康检查返回 200。
服务继续使用既有 `./data`/SQLite/master key，workers=false，监听 127.0.0.1:8080/8081。
当前 CUA 仅发现 Chrome 中的外部页面，没有发现可复用的已登录本地 Admin tab，因此未在
浏览器中输入凭据或伪造登录；真实 binding 仍保持 `mimo-v2.5`，未发 Provider 请求。

补充核对：Admin 的手动登记接口对同一 Provider 下重复 `external_model_id` 返回
`409 model_exists`，不存在通过“重新登记同名模型”更新 capability 的路径；State
repository 的 `update_model` 仅有内部 application boundary，当前 Admin 没有模型编辑 API。
生成类 role 不再需要此入口来声明原生 JSON Schema；对 embeddings/reranking 仍需真实
capability。未擅自修改 `mimo-v2.6-flash` 的 capability JSON。

2026-09-22：确认 `rerank` 没有实际 Provider/runtime 操作，仅保留历史 State binding。
Admin 模型用途配置已移除 rerank 入口，未删除历史绑定或迁移数据；embedding 与生成角色
保持可配置。
