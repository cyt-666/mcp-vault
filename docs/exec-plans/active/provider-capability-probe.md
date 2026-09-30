# Provider 能力探针

状态：probe P1 修复完成；本地验证通过；2026-09-21 真实能力探针中 generation 检查通过、embedding 返回 `provider_http_error`；M6 仍待独立评测
创建日期：2026-09-20

## 目标与边界

实现独立的 `provider-capability-probe` CLI，执行一次结构化生成和一次 embedding 能力发现/连通性检查，输入固定为合成文本。它不运行 M6 语义或任务质量评测，也不构成上线、生产数据读取或 M6 通过证明。

探针复用 `ProviderService`、`AuthService`、`StateStore` 和 `ManagedVaultService`。它不启动 Server、Worker、MCP 或 Admin，不使用 Provider model discovery，也不写向量记录。

目标隔离 State 只保存创建探针所需的 Vault 初始化、Provider/model 配置和新 key 加密后的 credential。source State 只读。探针不会写 embedding/vector 记录、语义卡片或 M6 评测结果。

## 安全约束

- 只有精确旗标 `--run-authorized-provider-capability-probe` 才能进入执行路径。没有旗标或配置时，在打开 State、master key 或创建 Provider service 前退出。
- 所有目标根和 master-key 路径由配置显式提供，必须为绝对、规范、symlink-safe 路径，并位于独立 `run_root` 中。根之间不得重叠；拒绝 `data`、`vault`、`production` 路径成分。
- Unix 上新建的 `run_root`、目标子目录和 key parent 显式设为 `0700`，新 master key 与 checkpoint 显式设为 `0600`。启动前拒绝 run root、已有目标 key/parent 或 source key/parent 的 group/other 权限位。非 Unix 平台没有已实现的 ACL 适配，因此执行 fail closed。
- 目标 State、Vault、history、artifact 和 master key 必须为全新隔离资源。执行根必须为空；目标 Provider 使用新生成的 master key。
- 可选 credential provisioning 分别为 generation 和 embedding 声明 source DB、source key 与 source Provider ID。source State 只读；source `AuthService` 仅在内存中解密；destination `ProviderService::create_provider` 使用新 key 重新加密。不得复制 source ciphertext、Vault、State、默认 key 或明文 secret。
- source Provider/secret 是安装级全局对象，不虚构 Provider→Vault 归属。兼容读取 API 仅按显式 Provider ID 查询旧 schema 中稳定的 `id`/`secret_id`，随后继续由 Auth 按 purpose/owner AAD 读取 secret；不调用依赖 `embedding_revision` 的完整 Provider 配置查询。
- credential provisioning 可只读接收最高 migration 11、包含 Provider secret reference 与所需 Auth secret metadata 列的 source State。此兼容范围仅覆盖该安装级 reference/secret 读取；不迁移、不写 source State，也不复制 source Vault 数据。M6 evaluator、目标 State 及其他正常 State 操作仍要求当前 schema。
- 对 source reference 不存在/缺少 secret 和 schema 不兼容分别记录 `source_provider_reference_unavailable` 与 `source_provider_schema_incompatible`；Auth metadata/AAD 和 decrypt failure 仍使用独立稳定错误码。checkpoint 只含安全错误码。
- Provider kind、Base URL、外部 model ID 与 capabilities 必须由操作者显式配置。embedding expected dimension 使用 `Known(positive)` 或 `Discover` 显式模式；Known 必须与 model capability 中明示的 dimension 一致，Discover 要求 model capability dimension 留空。Preflight 不进行网络访问或 model discovery。Generation 的唯一成功输出为严格对象 `{"ok":true}`；false、缺字段、类型错误和额外字段均失败，且不调用 embedding。
- Provider Transport 的 `max_retries` 固定为 `0`。generation 与 embedding 共用不可调高的原子 request budget `2`。generation 失败后停止，不调用 embedding。已有 checkpoint 会阻止重跑，避免 crash 后重复发出潜在计费请求。
- Generation 探针固定请求 256 个 token，并为支持的兼容预设显式关闭 thinking；低于 256 的 generation capability 上限在 preflight 拒绝。该策略只作用于探针隔离 State，不影响正常 MCP 或 M6 evaluator 的模型设置。
- checkpoint 只保存 stage、status、Provider kind、model ID、request count、稳定 error code、可选数字 `http_status`、安全 usage 状态、实际 dimension 和 dimension_status。仅 `ProviderError::HttpStatus` 会写入数字状态码；其他错误保持 `null`。Discover 只断言实际 dimension 大于零，并记为 `discovered`；绝不将实际值回写到 source/target model capability。checkpoint 经同目录临时文件与 rename 原子更新；不保存 endpoint、header、credential、prompt、output、response body、vector 或文本。缺少新 `http_status` 字段的旧准备 checkpoint 会被拒绝。

## 实现范围

- [x] 新增 probe config、preflight path validation、固定合成输入、Provider application boundary 和 CLI。
- [x] 使用新 State/schema、Managed Vault、空 history/artifact 根与新 master key。
- [x] 新增共享硬 request cap、禁用重试、阶段 checkpoint 和失败停止逻辑。
- [x] 新增按 generation/embedding 分别显式 source credential 的应用层重加密路径。
- [x] 新增只读安装级 Provider secret-reference 查询，兼容 current 与 migration 11 schema；probe 不再调用完整 Provider 查询，并保留 Auth purpose/owner AAD 验证。
- [x] 新增 current/migration 11 reference fixture 测试，证明旧 schema 下完整 Provider 查询失败而窄 reference 与 probe secret-read 路径成功；source State 只读且零写。
- [x] 新增本地 fake 测试：preflight 不触发请求或创建 run root、不安全权限/symlink 被拒、generation failure 或非精确 `{"ok":true}` 不执行 embedding、成功一次各阶段、cap/重跑/脱敏/密钥隔离和向量无写入。
- [x] 新增显式 Known/Discover 维度契约；fake 覆盖 Discover 正维度、Discover 零值与 Provider 错误停止、Known 不匹配；同步 config schema 与 runbook，标明能力发现不是 M6 通过。
- [x] 运行相关 providers/auth/state/eval/server 测试、fmt、clippy 与 diff 检查，并记录本轮共享工作区中的 M6 验证边界。

## CLI 与操作者输入

只做本地配置与隔离路径检查时，可使用：

```text
cargo run -p mcp-vault-eval --bin provider-capability-probe -- \
  --preflight-provider-capability-probe /absolute/path/probe-config.json
```

该模式不创建根目录、不打开 State/key、不构造 Provider，也不发出请求。实际运行入口：

```text
cargo run -p mcp-vault-eval --bin provider-capability-probe -- \
  --run-authorized-provider-capability-probe /absolute/path/probe-config.json
```

配置文件只包含路径、Provider 身份和非秘密模型配置。不要把 API key 写入 JSON、命令行参数或环境变量。若使用 source credential provisioning，配置仅引用 source installation 的 canonical State DB/key 路径和 Provider ID；探针从 source 加密存储中解密，再在目标安装内重加密。

执行前需要操作者提供：

1. 新建且为空的绝对 `run_root`，以及其中互不重叠的 `state_root`、`vault_root`、`history_root`、`artifact_root` 和未存在的 `master_key_path`。
2. 新 Vault slug 与明确的 `provider_mode`。
3. generation 与 embedding 各自的 Provider name、kind、Base URL、外部 model ID 和 capabilities。结构化输出与 embedding 支持均由操作者明示；embedding 通过 `embedding_expected_dimension` 选择 Known（同时在 capabilities 明示同一维度）或 Discover（capabilities dimension 为 null）。
4. Provider 凭据：可不提供 secret（例如不需认证的显式本地端点），或为 generation/embedding 各自提供 source DB/key/Provider ID。不得把两阶段凭据隐式共享。
5. 对相应 endpoint 的单次请求授权与费用接受。成功最多发出两次请求；失败不会自动重试。

Unix 上 source master key 必须由当前操作者可读，且 key 文件与 parent 不得向 group/other 开放。不符合要求时，应由操作者先修正 source key 权限。

详细 JSON 字段与隔离要求见 [Provider 能力探针操作说明](../../runbooks/provider-capability-probe.md)。探针成功只证明两个固定合成请求在当时返回了结构化结果与匹配/发现的正维度；这是一次能力发现，不是 M6 通过。M6 仍需独立的真实任务、质量指标、人工复核与费用报告。

## 验证记录

本节记录实现阶段的本地验证；2026-09-21 真实 Provider 能力探针执行记录见文末。

- `cargo fmt --all --check`：通过。
- `git diff --check`：通过。
- `cargo test -p mcp-vault-eval --lib provider_capability_probe`：Unix 本地 14 项通过，包括 Discover 正维度、Discover 零值/Provider 错误、Known mismatch 与 capability 配置一致性。
- `cargo test -p mcp-vault-eval --all-features`：60 项通过，包括 eval unit 27、app-boundary 3、live-runner 25、M6 fixture 4 和 shadow execution 1。
- `cargo test -p mcp-vault-providers -p mcp-vault-auth -p mcp-vault-state`：全部通过，分别为 auth 29、providers 27、state 83 项测试。
- `cargo check -p mcp-vault-eval --bin provider-capability-probe`：通过。
- `cargo clippy -p mcp-vault-eval --all-targets --all-features -- -D warnings`：通过。
- 上述实现阶段验证未运行 probe CLI、生产服务、外部 Provider 或真实数据。Provider crate 测试使用仓库自带的本地 fake/loopback contract fixture，不访问外部服务。

### Legacy source Provider-reference 修复验证（2026-09-20）

- `cargo test -p mcp-vault-state narrow_reference_query --lib`：2 项通过。测试分别使用当前迁移和精确到 migration 11 的嵌入迁移 fixture；旧 schema 的完整 `get_provider` 查询因缺少 `embedding_revision` 失败，窄查询成功；缺少稳定列时返回 schema-incompatible 类型错误。
- `cargo test -p mcp-vault-eval --lib`：30 项通过。新增旧 Provider 表 source fixture 证明 read-only probe secret 路径不调用完整 Provider 查询，Auth purpose/owner AAD 验证与合成 secret 解密成功，source DB 长度和修改时间不变；reference 缺失与 schema 不兼容返回不同稳定错误码。
- `cargo test -p mcp-vault-auth -p mcp-vault-state --all-features`：通过；auth 29、state unit 31，以及 auth/background/repository/semantic integration suites 全部通过。
- `cargo clippy -p mcp-vault-auth -p mcp-vault-state -p mcp-vault-providers -p mcp-vault-eval --all-targets --all-features -- -D warnings`、`cargo fmt --all --check`、`git diff --check`：通过。Clippy 编译 Providers 所有目标，但未执行其 loopback HTTP/Reqwest 测试。
- 本修复验证没有运行 probe CLI、Provider 请求、网络服务或生产路径，也没有读取真实 key、secret、ciphertext 或 Vault 内容；测试中仅构造临时合成密钥与 ciphertext。

### Prepare/run 状态机修复（2026-09-20）

- [x] 将 preflight 限定为新建/空 destination 校验；只检查文件系统元数据与目录空性，不打开 State 或读取 destination key 内容。
- [x] prepared run 独立验证 prepare 写出的 key、HMAC 配置/身份封印、私有目录树、State 完整性、唯一 Vault、provider mode，以及两组 Provider/model 的 ID 与完整配置一致性；允许 prepare 自己创建的 key/state。
- [x] 将配置漂移或 prepared filesystem/State validation error 原子记录为 `provisioning/failed` 与稳定错误码；重跑仍拒绝，失败时 Provider fake 也保持零调用。
- [x] fake generation 测试覆盖 prepared key 可用、成功与重放拒绝、配置漂移、unexpected root entry、无 Provider 调用；source legacy read-only 与 target key re-encryption 测试继续通过。

本轮检查：`cargo fmt --all --check`、`git diff --check`、`cargo test -p mcp-vault-eval --all-features`、`cargo clippy -p mcp-vault-eval --all-targets --all-features -- -D warnings`、`cargo test -p mcp-vault-auth -p mcp-vault-state --all-features` 均通过。没有运行 probe CLI、外部 Provider 或网络请求；eval fake 边界和合成 source/destination State 仅在本地测试中使用。

### Probe generation thinking 与 token limit 修复（2026-09-20）

- [x] Generation 探针使用私有模型设置，固定 generation token limit 为 256，并对支持的兼容 Provider 显式关闭 thinking；embedding 仍使用默认模型设置。Prepared-State 校验逐模型验证对应设置。
- [x] capability 明示的 generation `max_output_tokens` 若低于 256，preflight 返回配置错误，避免有效上限被静默压低。runbook 与 MiMo 本地 preflight JSON 示例同步为 256。
- [x] 新增 loopback HTTP 完整 prepare/run 回归，捕获实际 generation wire payload 并断言 `thinking.type=disabled`、`max_completion_tokens=256`，同时走过 prepared-State 校验和一次 embedding；没有外部网络或 Provider 调用。

验证：`cargo test -p mcp-vault-eval --all-features -q` 全部通过（35 个单元、3 个 app-boundary、25 个 live-runner、4 个 M6 fixture、1 个 shadow execution）；`cargo clippy -p mcp-vault-eval --all-targets --all-features -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 通过。未运行 probe CLI 或读取本地配置所引用的 State/key/Vault。

### 真实 Provider 能力探针执行记录（2026-09-21）

- 离线 preflight 通过，输出 `preflight_ok`；Cargo 使用 offline 模式。真实执行只启动一次显式授权的 `--run-authorized-provider-capability-probe` 命令。
- Generation 阶段通过严格对象 `{"ok":true}` 校验并进入 embedding 阶段；usage 状态为 `reported`。checkpoint 只保留安全状态字段，不含输出或响应正文。
- Embedding 仅尝试一次后失败。checkpoint 为 `stage=embedding`、`status=failed`、`request_count=2`、`error_code=provider_http_error`、`dimension=null`、`dimension_status=null`、`usage_status=reported`。Transport 重试数为 0；没有第二次运行。
- 该次执行发生在 `http_status` 字段加入前；旧 checkpoint 只保存了 `provider_http_error`，具体状态码无法回溯，记录为 `provider_http_error only, cause unknown`。新字段仅用于之后的探针执行，不补写本次结果。
- 本次探针整体失败，不证明 embedding 可用；M6 仍待单独授权和完整质量评测，当前不得据此进入 M6。隔离 run root 与 checkpoint 均保留。

### 本地 Provider/embedding 配置元数据审计（2026-09-21）

- 以 SQLite read-only 连接并启用 `query_only`，按当前 Vault override/global fallback 只聚合 `memory_extraction` 与 `embedding_memory` 绑定及 Provider kind、model capability 布尔值；没有读取或输出资源 ID、endpoint、secret reference、key、模型请求/响应或 Vault note 内容。
- 当前唯一 Vault 的两个有效角色绑定到同一个模型，与此前 probe 使用的 MiMo generation model 相同。该模型 metadata 明确为 `structured_output=false`、`embeddings=false`。安装内六个已注册模型的 `embeddings` capability 均明确为 false；没有另一个已注册且显式支持 embedding 的模型，也没有专用 embedding model 配置。
- 这些结论只描述本地配置 metadata，不代表 Provider 运行时能力。此前 HTTP 状态码仍不可回溯；本审计不推断该次 HTTP 失败原因，也未运行 Provider 或 M6。
