# Provider 能力探针操作说明

## 简介

`provider-capability-probe` 是一次受控的 Provider 能力发现/连通性检查。它用固定合成输入执行一次结构化生成和一次 embedding。它不读取真实笔记，也不评估语义检索或答案质量；成功只说明本次配置下两个操作可调用，不代表 M6 通过。

只有显式授权旗标才会执行探针。配置缺失、旗标错误或隔离校验失败时，工具必须在打开 State、master key 或创建 Provider service 前退出。独立的 `--preflight-provider-capability-probe` 模式只检查配置，不创建目录、不打开 State/key、不构造 Provider，也不发出请求。

## 前置条件

- 准备一个专用、绝对且规范的 `run_root`。该目录必须为空，不得放在 `./data`、生产、Vault 或已有服务状态目录下。
- `state_root`、`vault_root`、`history_root`、`artifact_root` 和 master key 路径必须互不重叠，并位于 `run_root` 下。Vault 目录固定为 `<run_root>/vaults/<vault_slug>`。
- 路径本身及现有父级不得经过 symlink。所有路径都必须使用 canonical 形式；不要依赖 `..`、`.` 或系统临时目录的 symlink 别名。
- Unix 上 `run_root`、所有隔离子目录及 destination key parent 必须没有 group/other 权限位。新建目录使用 `0700`，master key、SQLite 状态文件、checkpoint 与准备封印文件使用 `0600`。工具不会放宽已有路径权限。
- Provider 凭据使用明确的 source installation 配置时，source DB 和 key 路径必须 canonical。source master key 文件必须 owner 可读；key 文件及 parent 必须屏蔽全部 group/other 权限位。source Provider ID 必须精确对应有权使用的 Provider。
- 确认 endpoint 支持配置的模型和操作。不要依赖探针自动发现 model，也不要把 URL query 或 URL userinfo 用作 credential。

非 Unix 平台尚无可验证的私有 ACL 实现。当前 probe 会 fail closed；不要通过复制或放宽权限来绕过。

## 配置

保存为独立 JSON 文件。下面是结构示意；请替换所有 `<...>` 值。JSON 不接受未知字段，也不得包含 API key、Authorization header 或任何明文 secret。

```json
{
  "schema_version": "provider-capability-probe-v2",
  "run_root": "/absolute/canonical/path/probe-run",
  "state_root": "/absolute/canonical/path/probe-run/state",
  "vault_root": "/absolute/canonical/path/probe-run/vaults/m6-probe",
  "history_root": "/absolute/canonical/path/probe-run/history",
  "artifact_root": "/absolute/canonical/path/probe-run/artifacts",
  "master_key_path": "/absolute/canonical/path/probe-run/keys/master-key",
  "vault_slug": "m6-probe",
  "provider_mode": "enabled",
  "generation": {
    "name": "m6-probe-generation",
    "kind": "deepseek",
    "base_url": "https://<provider-api-root>/v1/",
    "model_id": "<registered-external-model-id>",
    "capabilities": {
      "structured_output": true,
      "embeddings": false,
      "reranking": false,
      "dimension": null,
      "context_window": null,
      "max_output_tokens": 256
    }
  },
  "embedding": {
    "name": "m6-probe-embedding",
    "kind": "embedding_http",
    "base_url": "https://<provider-api-root>/v1/",
    "model_id": "<registered-external-embedding-model-id>",
    "capabilities": {
      "structured_output": false,
      "embeddings": true,
      "reranking": false,
      "dimension": null,
      "context_window": null,
      "max_output_tokens": null
    }
  },
  "embedding_expected_dimension": {
    "mode": "discover"
  },
  "credential_provisioning": {
    "generation": {
      "source_database_path": "/canonical/source/state.sqlite3",
      "source_master_key_path": "/canonical/source/master-key",
      "source_provider_id": "<generation-source-provider-uuid>"
    },
    "embedding": {
      "source_database_path": "/canonical/source/state.sqlite3",
      "source_master_key_path": "/canonical/source/master-key",
      "source_provider_id": "<embedding-source-provider-uuid>"
    }
  }
}
```

`embedding_expected_dimension` 必须显式选择一种模式。若选 `{"mode":"known","dimension":2048}`，`dimension` 必须为正数，且必须与 embedding `capabilities.dimension` 中明示的操作者配置相同；真实返回值仍须严格相等。若选 `{"mode":"discover"}`，embedding `capabilities.dimension` 必须为 `null`。探针只验证真实返回维度大于零，并将返回值记入 checkpoint 的 `dimension`、`dimension_status: "discovered"`。它不会把发现值写入目标或 source State 的 model capabilities，也不会改变配置文件。结构化输出、embedding 等其他 capability 仍由操作者显式声明，探针不会自动发现或补写。

若为当前绑定的 `mimo-v2.5` 准备 generation 探针配置，应将确切 Provider kind、model ID、Base URL 与 `structured_output` capability 作为操作者配置显式写入。该受控探针只检查这一结构化 generation 调用和 embedding 维度；即使两者成功，也不能标记为 M6 通过。

Generation 探针请求固定使用 256 个 token 的上限，并在支持兼容预设的模型上显式关闭 thinking；注册 capability 若设置了更低的 `max_output_tokens`，preflight 会拒绝配置，避免实际请求被静默压低。此项设置仅用于隔离探针，不改变服务中正常 MCP generation 或 M6 evaluator 的模型设置。

`credential_provisioning` 是可选字段。若不传，destination Provider 不配置 secret。若传入，只会从 source State 的加密 Provider secret 读取；source State 以只读模式打开。Provider 与它的 secret reference 是安装级全局对象，`source_provider_id` 必须显式指定。读取路径只查询稳定的 Provider `id`/`secret_id`，然后由 source `AuthService` 按 Provider purpose、owner type 和 owner ID AAD 验证并在内存中解密；不会读取或复制 Provider 配置、source ciphertext、Vault 内容或其他 State。source key 只在进程内使用。探针通过目标 `ProviderService` 新建 Provider，并使用目标 master key 重新加密 secret。generation 与 embedding 各自独立声明 source；不要假设两者 credential 相同。

source State 的当前完整 Provider 查询依赖较新的 schema。为读取这一安装级 Provider secret reference，允许只读使用最高为 migration 11、且包含 `providers.id`/`secret_id` 与 Auth secret 元数据列的 source State；不执行 source migration 或写入。reference 不存在/无 secret 与 schema 不兼容分别报告为 `source_provider_reference_unavailable` 和 `source_provider_schema_incompatible`，checkpoint 不包含 secret 或数据库细节。这个窄例外不适用于 M6：M6 evaluator、探针目标 State 及其他正常 State 操作仍要求当前 schema。

执行探针时，`provider_mode` 必须明确选择 `enabled`；`disabled` 会在发送前拒绝调用。旧值 `local_only` 读取为 `disabled`，`remote_allowed` 读取为 `enabled`。安装管理员负责配置目标地址，Provider Transport 使用普通直连或环境代理，不预解析或限制目标 IP。HTTPS 保持证书验证，显式配置的 HTTP 不提供 TLS 保护；所有重定向均被拒绝。探针不会修改 Provider headers，也不接受 URL 内嵌 secret。

## 执行

先运行不接触 State/key、不联网的 preflight：

```text
cargo run -p mcp-vault-eval --bin provider-capability-probe -- \
  --preflight-provider-capability-probe /absolute/path/probe-config.json
```

preflight 返回 `{"status":"preflight_ok"}` 表示本地配置与路径通过。它不验证 endpoint、凭据、Provider 模型可用性或远端 capability。

确认配置后，再运行经授权的真实 Provider 探针：

```text
cargo run -p mcp-vault-eval --bin provider-capability-probe -- \
  --run-authorized-provider-capability-probe /absolute/path/probe-config.json
```

缺少旗标、传入其他旗标或缺少配置路径都会拒绝运行。执行路径会创建新的目标 State、空 Vault、空 history/artifact 根，以及目标 master key；创建 Provider 与手工注册的 model 配置。工具不启动 server、worker、MCP 或 Admin。

目标隔离 State 会保存 Vault registry/初始化任务、Provider/model 配置和目标 key 加密的 secret 元数据。source State 保持只读。由于没有启动 Worker，Vault 初始化任务不会被消费。探针不会写 embedding/vector 投影、语义记忆或 M6 评测结果。

准备完成后，运行阶段只接受 prepare 创建的私有目录树、目标 key、当前 schema 的只读可校验 State、单一配置 Vault、两组匹配的 Provider/model 记录，以及仍处于 `provisioning/running`、请求数为零且 `http_status` 为 `null` 的 checkpoint。缺少 `http_status` 字段的旧 checkpoint 不会作为当前准备状态接受。Artifact 根还包含一个 `0600` 的内部准备封印；它用新 destination key 认证完整配置与创建出的 Vault/Provider/model 身份，运行时会重新校验。任意已有 key/State、配置漂移、意外文件、symlink、hardlink 或权限变化都在发出 Provider 请求前拒绝，并尽可能原子写入 `provisioning/failed` checkpoint 与稳定错误码。

Transport retries 固定为 `0`。Generation 与 embedding 共享总计 `2` 次请求的原子硬上限。若 generation 失败，工具立即停止，不调用 embedding。已有 checkpoint（包括失败或终态 checkpoint）都会拒绝复跑，因此中断后的潜在重复计费窗口不会被自动重试。

## 结果与安全边界

checkpoint 位于：

```text
<artifact_root>/provider-capability-probe.checkpoint.json
```

checkpoint 仅包含 stage、status、Provider kind、model ID、请求数、稳定错误码、可选数字 `http_status`、安全 usage 状态、返回的 embedding dimension 和 `dimension_status`。只有 Provider 返回非成功 HTTP 状态时才记录数字状态码；普通传输、解析、配置或结构化输出错误将 `http_status` 留为 `null`。它不包含 endpoint、secret、header、prompt、Provider 输出、response body、vector 或文本。`dimension_status` 为 `matched` 表示 Known 模式相等，为 `discovered` 表示 Discover 模式成功；dimension 为零会失败并记为 `invalid`，Provider 错误则不记录维度。

Artifact 根中的隐藏准备封印只保存 key 派生的认证标签和本次隔离 State 的身份 ID，不保存配置字段或 key。不要手工编辑或复制该文件；它只用于证明当前 checkpoint、配置、Vault 与 Provider/model State 属于同一次 prepare。

Generation 只有返回精确对象 `{"ok":true}` 才算成功。`ok:false`、缺字段、字段类型错误、额外字段或 Provider schema 校验失败都会写 failed checkpoint，并停止在 generation 阶段，不调用 embedding。

Known 模式成功要求 embedding dimension 与显式配置一致；Discover 模式成功仅要求返回维度大于零。两种结果都只属于本次能力探针，不证明 M6 检索、语义卡片、无答案拒绝、来源支持、任务答案或整体质量通过。M6 真实任务评测仍需单独授权、冻结数据集、任务指标、费用与人工复核报告。

探针不会清理或覆盖 run root。完成或失败后应保留 checkpoint 和隔离 State 作为证据；如需新一轮探针，应由操作者指定一个新的空 `run_root` 和新的 key 路径，不要在本工具内自动删除旧数据。
