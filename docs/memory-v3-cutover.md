# v3 记忆系统切换手册

本文面向现有 MCP Vault 的维护者。新系统保留完整原文单元，停用旧正文生成、合并和数据转换路径。切换会抛弃全部旧记忆，包括明确记忆、自动记忆、正式合并结果、旧暂停与排除记录。

普通笔记、附件、文件历史、Vault、账号、凭据和 Provider／模型绑定继续保留。生产环境不在本次开发环境的可访问范围内；本文提供生产端执行步骤，本地验收不能替代生产验收。

## 切换前准备

先在现有管理端记录以下信息：

- 每个 Vault 的名称、状态和实际内容目录；
- SQLite、历史目录、安装主密钥和其他挂载目录；
- 当前数据面、管理面地址和模型用途绑定；
- 若干有代表性的笔记、附件与文件历史。

准备包含迁移 0028、0029、0030 和 `initialize-memory` 命令的新二进制或镜像。保留旧二进制／镜像和现有部署配置。先完成隔离副本验证，再进入生产维护窗口。

以下命令针对仓库根目录的默认 `compose.yaml`，服务名为 `mcp-vault`，数据位于 `./data`。自定义部署必须使用原有挂载和环境配置。特别是 Vault 内容不在 `/data` 内时，必须把对应外部目录一起备份并挂载给初始化进程。

## 首选：通过管理端初始化

更新到包含 Admin 初始化入口的镜像并正常启动服务。登录管理端，选择要处理的 Vault，打开“记忆”页面。页面会显示旧记录数量、受管旧文件数量和当前初始化阶段；新 Vault 没有旧初始化记录时直接显示 ready，不会创建清理任务。

确认备份完成后，点击“确认清理旧记忆”。确认请求通过 Admin 会话、Origin 和 CSRF 校验，立即返回后台任务状态。服务先进入受控维护状态并停止新的读写 admission，再在有限时间内排空已受理的操作；普通 Vault 内容、附件、历史和新记忆保留。页面会轮询阶段、清理进度和安全错误码。任务失败或服务重启后，页面显示可续跑状态，管理员必须再次确认“继续初始化”；服务不会在普通重启时自动清理。

初始化成功后，新记忆生成保持暂停。先完成登录、WebDAV 和 MCP 读写检查，再在同一页面点击“继续生成”。如果清理已进入 `clearing` 后留下维护状态，先用页面显示的错误和进度判断是否可续跑；不能手工把阶段改为 `ready`。若 canonical 状态已经是 ready、只有 task 元数据未终态，使用管理端“维护恢复”入口，服务只补 task 元数据，不重新清理文件。

管理端不可用或需要离线运维时，使用下面的 CLI 作为替代入口。CLI 仍要求服务停止和 SQLite 独占，不应与运行中的 Admin 初始化同时使用。

## CLI 前准备与成对备份

暂停 Obsidian／WebDAV 同步和 Agent 写入，停止旧服务：

```bash
docker compose stop mcp-vault
docker compose ps --status running mcp-vault
```

第二条命令不应显示正在运行的 MCP Vault 实例。确认没有其他服务实例或维护程序打开同一 SQLite。初始化器会进一步检查进程锁和 SQLite 独占锁；锁失败时应找到实际占用者，不要删除锁文件来绕过检查。

默认目录的备份示例：

```bash
backup_dir="../mcp-vault-before-v3-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$backup_dir"
tar -czf "$backup_dir/data.tar.gz" data
cp compose.yaml "$backup_dir/compose.yaml"
tar -tzf "$backup_dir/data.tar.gz" > "$backup_dir/archive-files.txt"
```

归档必须同时包含 SQLite 及仍存在的 WAL／SHM 文件、全部 Vault 内容、历史目录和安装密钥。额外挂载不在上面的 `data` 归档中，需要单独归档。按现有权限保护备份，不把凭据或正文写入工单和日志。

## 执行离线初始化

如果初始化失败，先不要重跑清理命令。保持服务停止，用只读诊断查看每个 Vault 的 phase、已完成文件数、旧记录计数和未完成 journal 阶段：

```bash
docker compose run --rm --no-deps mcp-vault initialize-memory --inspect
```

`--inspect` 只打开已存在的 SQLite 只读连接，不创建数据库、不迁移、不执行 Core recovery、不删除文件、不调用 Provider，并正常读取 SQLite WAL 中已提交的状态。输出不包含 journal payload、正文或凭据。

如果使用仓库源码构建，先构建新镜像，然后启动一次性初始化进程：

```bash
docker compose build mcp-vault
docker compose run --rm --no-deps mcp-vault initialize-memory --discard-legacy-memory
```

如果采用预构建镜像，先把部署配置指向已验证的固定镜像，再执行第二条命令。不要先启动新服务处理业务流量。

直接部署二进制时，在与原服务相同的环境配置和工作目录下运行：

```bash
mcp-vault initialize-memory --discard-legacy-memory
```

初始化进程不启动监听器，也不调用 Provider。它先输出文件路径与记录数量组成的清理预览，随后执行以下步骤：

1. 独占打开数据库，执行前向迁移并检查完整性。
2. 在每个需要初始化的 Vault 中保存持久清理清单。
3. 不重放旧 `memory/` namespace 的 Core 写入意图；只有确认每条未完成 journal 的所有路径都在该 namespace 内，才将其终结为 `discarded` 并清理 journal-owned temporary。跨边界或无法分类的 journal 会阻止初始化。
4. 通过 Vault Core 按持久清单中的 hash 删除旧 `memory/` 受管命名空间文件，保留必要历史。
5. 清除清单允许的旧记忆表、任务、向量、配置与排除状态。
6. 记录完成状态，并为新生成流程设置维护暂停。

新 `memory-v3/` 文件不属于清理目标。普通启动不会重复清零。已经完成初始化的 Vault 返回 `already_initialized`；重新执行不会删除新单元。原本禁用的 Vault 仍保持禁用。

成功输出的顶层 `contract` 为 `source_preserving_memory_units_v3`。各 Vault 的 `outcome` 为 `initialized` 或 `already_initialized`。首次切换完成后，`generation_paused` 应为 `true`。

## 中断后继续

初始化中断时，保持旧服务和客户端写入暂停，用相同配置重新执行同一命令。程序读取已保存清单，继续终结范围内尚未处理的旧 journal，并重新核对文件哈希；已删除文件按幂等方式处理。当前 generation 产生的文件删除 journal 也作为 legacy-scope intent 处理后按原清单重试。

若程序报告文件发生变化、需要人工检查或完整性失败，先保留日志与备份。核对导致失败的文件和实际挂载，再决定继续恢复还是回滚。不要手工把初始化阶段改成 `ready`，也不要删除 SQLite 表来制造成功状态。

## 启动并检查保留数据

```bash
docker compose up -d mcp-vault
docker compose logs --tail 100 mcp-vault
```

在恢复业务写入前逐项检查：

- 使用原账号登录管理端；核对 Vault 名称、状态、Provider 和模型用途绑定。
- 通过原 WebDAV 地址读取笔记和附件，核对正文或文件哈希；检查旧文件历史。
- 使用原 PAT 或 OAuth 连接执行 `tools/list`、`read_note`。
- 确认存在 `get_memory_overview`，旧合并、校准和迁移管理路由不再出现。
- 新建一条明确记忆，使用 `get_memory` 核对完整正文；按返回的修订更新，再删除。
- 查看管理端“记忆生成与概览”，确认新生成处于维护暂停。

OAuth 客户端是否需要重新发现工具取决于 Host 的缓存行为。凭据未被清除，并不意味着每个 Host 会立即刷新工具清单。

## 开放服务并重新生成

保留数据和协议检查通过后，恢复业务服务。在管理端点击“继续生成”。这会解除新维护暂停，并对现有普通笔记安排全量处理；旧暂停和排除记录不会继承。

自动选择使用原有 `memory_extraction` 绑定。`memory_overview` 是可选用途；未配置时使用提取模型。Provider 数据发送策略和模型就绪状态仍需有效。没有可用模型时，明确记忆和关键词读取可用，自动生成等待配置修复。

长笔记按完整单元分批处理，批次结果持久保存，全部批次结束后才发布完整来源集合。超限或含敏感内容的单元在管理端单独报告，仍可通过普通笔记检索读取。每批默认请求期限为 300 秒。

随后检查自动单元与原文的关系、来源坐标、超限提示、模型输出失败和任务完成状态。生成的概览只是导航说明；实际记忆正文仍来自原文。通过 `recall` 和 `get_memory_overview` 抽查常用任务，并检查默认预算下的完整正文与超长单元读取入口。

## 回滚边界

在重新开放写入之前，可以停止新服务，恢复同一时间点的 SQLite、Vault、历史目录与密钥，并切回旧二进制／镜像。恢复必须成对进行，不能只恢复数据库或只恢复受管记忆文件。

一旦已经接收新写入，旧整库备份就不再包含这些变化。此时不能直接覆盖回滚；先保存新数据，再制定差异恢复方案。旧备份仅用于运维恢复，不接入新系统的模型可读路径。

## 本地与生产验收

本地工程检查、真实模型语料验收和生产验收分别记录。真实模型成功处理语料不等于每次选择都正确；假 Provider 的契约测试也不能证明语义质量。

本次生产端尚未执行切换。维护者完成上述步骤后，应保存初始化结果、保留数据核对、协议读写和全量生成结果，作为生产验收依据。

设计依据：[ADR-0033](adr/0033-source-preserving-memory-units.md)、[记忆系统契约](memory-system.md)、[数据模型](data-model.md)和[部署文档](deployment-and-operations.md)。
