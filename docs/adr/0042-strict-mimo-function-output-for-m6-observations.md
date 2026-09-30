# ADR-0042：M6 observation 使用 MiMo strict function output

- 状态：已接受
- 日期：2026-09-29
- 修订：仅改变隔离 M6 evaluation 的 MiMo observation Provider wire；production extraction 和其它 Provider 默认路径不变。

## 背景

R1–R4 的失败均发生在 observation 阶段、JSON 已解析之后，但错误字段不同：R1/R2 是 source-local evidence enum mismatch，R3 是 assertion-status enum mismatch，R4 是 observation item type mismatch。每个run都由同一请求schema在本地严格拒绝且终态清理；没有读取原始模型输出，不能解释某个失败 token。静态检查未发现本地动态schema/输入catalog漂移。MiMo `json_object`只保证语法，不强制请求中的字段层级或类型；重复提示修复不构成可靠的结构保证。

官方[MiMo Structured Output说明](https://mimo.mi.com/docs/en-US/quick-start/usage-guide/text-generation/structured-output)指出`json_object`只保证JSON语法；[MiMo Chat API](https://mimo.mi.com/docs/en-US/api/chat)暴露function `strict` schema选项，但仅保证其声明支持的schema子集，且tool choice只依赖`auto`，不能假设forced named-tool choice有效。因此工具响应只是上游结构约束，仍需完整本地schema与业务验证。

## 决策

M6 observation prompt/schema升级为 `semantic-cards-tracked-adr-m6-v11` / `semantic-cards-m6-json-v7`。Eval对MiMo observation请求显式启用strict function-call wire；其它Provider、legacy extraction、composition、relation、answer以及production memory extraction仍使用原Provider structured-output默认路径。请求只提供一个function tool，`strict=true`、tool choice=`auto`；不传forced choice。模型返回的SSE `tool_calls` name/arguments fragments由Provider adapter有界聚合，必须只有choice 0与一个function index 0、function name精确匹配、finish reason为`tool_calls`且arguments非空。只有null/空content可与工具delta共存；任何非空content、no-tool、错名、多tool/不同index、工具ID漂移、截断、重复finish或畸形arguments都fail closed。不会把普通JSON文本当function arguments接受。

v7要求所有 observation 字段出现。`conditions`、`exceptions`、`ordered_steps`、`context_block_indices`空数组表示该类信息不存在；`result`、`uncertainty`空字符串表示该可选文本不存在；`source_time_scope`必须为对象，`status=unknown`时仅允许空`value`与空证据索引数组，`source_stated`时必须有非空value与可验证索引。Eval adapter仅按这些精确定义，将空标记规范化为旧内部Observation表示；不把任意空白文本、缺字段或无效状态推断为unknown/None。

Provider提交的strict function schema只使用基础对象、数组、string、integer、required、enum、`additionalProperties=false`；从上游schema移除其subset未承诺的`minItems`/`minimum`提示约束，原始完整v7 schema仍由本地Provider validator验证，Memory service再执行namespace/index、重复引用、Vault/source/revision/span/hash fences。若MiMo拒绝schema、返回普通文本、不调用tool或参数不合法，当前extraction失败并清理prepared work；保持zero retries，不进行repair请求。

## 限制与成本

MiMo支持的JSON Schema子集未被文档逐项列明；服务可能拒绝某些合法基础组合或模型可能在`auto`下不调用tool。严格工具模式提高上游结构约束，但不保证tool必被选择或语义质量通过。任何不确定响应都保留失败而不放宽本地验证。SSE聚合最多16,384个tool-call fragments、1 MiB arguments、64字节function name、128字节call ID；SSE transport原有响应总字节/单event限制同时适用。

## 验收

- 单次v11 M6 observation请求使用单个strict function、auto选择且不包含`response_format=json_object`；ordinary MiMo JSON-object和其它Provider请求wire不变。
- fragment拼接、function name/id一致性、choice/tool index、mixed content、no-tool、finish reason、字节/fragment上限与本地schema fail-closed有fake SSE覆盖。
- v7空值标记逐项规范化；未知值、未知字段、schema错值、namespace/index错误仍失败，旧v6 semantic validator及Memory证据fence继续执行。
- 在下一次完整M6前先用全新隔离root做B/S17或B/S18单来源真实diagnostic验证strict function wire；通过后才开另一个fresh full M6 root。保留固定source split、MiMo Flash、budget=160、retries=0和locked gold hash。

## 2026-09-29 修订：工具调用禁用思考并记录安全协议类别

D8在新的隔离B/S17诊断中以1个Provider请求终止，安全报告仅为 `provider_response_invalid`，没有schema path或JSON parser diagnostic，任务评分数为0。该泛化错误此前无法区分未调用工具、finish reason不符、tool delta畸形或选择/index错误。R4/D8封存的模型设置为MiMo preset/thinking `auto`；固定adapter会将MiMo `auto` 解析为 `thinking.type=enabled`。MiMo官方[API Integration FAQ](https://mimo.mi.com/docs/en-US/quick-start/faq/api-integration)指出thinking开启时工具调用可能进入reasoning内容并呈现不稳定/不完整，建议工具调用关闭thinking。D8没有保留SSE原始事件或协议形状，因此这只是与失败相符的首要解释，不能声称已从D8证明具体响应形状。

对 `strict_function_call=true` 的MiMo请求，最终wire现在强制设置 `thinking.type=disabled`，并保持严格function schema、单工具、`tool_choice=auto`、完整本地JSON/Memory验证不变。该覆盖只由隔离M6 observation的精确版本调用；普通MiMo JSON-object请求、其他Provider、production memory extraction、composition和其他阶段继续使用原thinking设置和请求体。strict模式继续省略temperature以避免启用该覆盖后意外改变既有strict请求的其它参数。v11 prompt/v7 schema保持不变，因为字段与语义契约未变。

strict SSE适配器仅保留 `StrictFunctionCallIssue` 的受限枚举，例如 `no_tool_call`、`wrong_finish_reason`、`missing_tool_call_id`、`invalid_choice`、`mixed_message_content`、`wrong_tool_name`、`missing_arguments` 与畸形delta/事件类别。既不保留也不记录function name、arguments、message/reasoning内容、请求体或响应片段；泛化provider code保持 `provider_response_invalid`。D9单源诊断报告可同时给出这个安全类别，且仍fail-closed、zero retries、最多2个请求。

先完成fake SSE、请求体与diagnostic report回归及完整离线门禁；随后用fresh D9 B/S17单源诊断验证该wire。D9成功只证明一次strict工具协议通路可用，不构成M6语义质量通过；若正确禁用thinking后仍无tool或strict schema不被服务接受，不再盲开full R5。

## 2026-09-29 修订：strict M6 observation使用完整非流式function响应

D9在strict mode已显式禁用thinking后，以单次B/S17请求失败；安全分类为 `invalid_tool_delta`，但此前该类别覆盖多个delta shape错误。MiMo[Chat API](https://mimo.mi.com/docs/en-US/api/chat)列明完整非流式 `choices[].message.tool_calls` 响应形状，而流式文档只列delta各字段类型，没有明确承诺function子对象在每个partial fragment中的出现顺序/完整性。为去除这项不确定性，仅M6 strict-function observation切为 `stream=false`；普通MiMo及其他Provider继续原stream路径。strict请求继续使用单个function tool、`strict=true`、`tool_choice=auto`、thinking disabled和不发送response_format。

严格非流式响应必须有且只有一个choice及一个function tool、choice index为0、finish reason为`tool_calls`、message content只能缺失/null/空白、function type/name/id/arguments完整且满足上限；不接受文本JSON fallback、多工具、错误name/id、非空content、超限或非法参数。HTTP仍经现有JSON transport的一次性入口，复用endpoint/SSRF、鉴权、content type/status、并发、请求预算、请求和响应字节上限、connect与overall timeout；绕过transport retry loop，不重试。arguments只在内存传给原JSON parser和完整v7 schema/local Memory/Vault/source/revision/span/hash fences。普通MiMo JSON-object和其他Provider的stream/body形态不变。

新增provider协议错误仍使用safe typed issue enum，不保存tool名称、ID、arguments、message/reasoning正文或HTTP body；传入诊断report的 `protocol_issue` 仅可取枚举值。SSE `tool_call.type` 若出现则必须是string `function`，修复此前非字符串值被忽略的fail-open分支。SSE原有边界测试继续覆盖普通stream；strict M6 HTTP fake现在使用MiMo官方完整非流式形状，并覆盖合法调用、no-tool、多工具、错误name、非法JSON arguments、混合content、超限、错误finish及缺ID。`request_json_once` fake验证即使transport常规重试配置非零也只发送一次。此改动不改变prompt/schema，仍为v11/v7。

最终验收前完成fmt、全workspace Clippy/tests、diff check；随后fresh D10 B/S17单源诊断最多2请求、0 task scoring。一次成功只证明strict wire capability，不代表M6语义质量通过；若thinking disabled+nonstream完整响应契约下仍失败，应基于D10安全类别判定Provider能力并停止无期限重试，不开full R5。
