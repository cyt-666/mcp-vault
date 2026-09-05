# MCP Vault

MCP Vault 是一个自托管的 Markdown 知识库和长期记忆服务。
它让人通过 Obsidian 管理同一个 Vault，也让 AI Agent 通过 MCP 发现、检索、
回忆和安全修改这些内容。

当前版本：<code>0.2.1</code>（2026-09-05）。部署示例默认使用
<code>mcp-vault:0.2.1</code>，目标架构为 <code>linux/amd64</code>。

## 项目定位

MCP Vault 面向希望长期保存个人知识、项目上下文和 Agent 决策的人。它保留
普通 Obsidian Vault 的可移植性，不把笔记转换成只能由服务读取的数据库记录。

项目遵循以下原则：

- Markdown 笔记、附件和已提交的语义记忆是知识的规范副本；
- SQLite 只负责凭据、配置、修订、任务和审计等运行状态；
- 全文索引、向量、主题和记忆投影都可以删除并重建；
- WebDAV、MCP 和 Admin 是三个独立的认证边界；
- 每一次数据操作都绑定一个 <code>VaultContext</code>，协议处理器不能自行访问文件或
  执行 SQL；
- 同一管理员可以创建多个服务托管 Vault；每个 Vault 使用独立的 WebDAV/MCP 链接、
  凭据、任务、索引和长期记忆；
- 远程模型是可选增强，不会阻塞 WebDAV、文件写入、词法搜索和已有记忆回忆。

MCP Vault 不是在线笔记编辑器，不是简单的文件系统 MCP 包装器，也不是用专有
数据库替代 Obsidian 的同步插件。

## 核心能力

| 能力 | 说明 |
| --- | --- |
| Obsidian 同步 | 通过标准 WebDAV 同步 Markdown、附件和 <code>.obsidian/</code> 文件 |
| 知识发现 | 提供 Vault 概览、目录索引、主题、标签、链接、反向链接和最近变更 |
| 检索 | 支持全文搜索；配置 embedding 后支持语义和混合检索 |
| 安全写入 | 创建、替换、追加、补丁、移动、删除、历史和恢复都经过 Vault Core |
| 长期记忆 | 通过两阶段提取与合并生成可检查、可追溯的 Markdown 记忆 |
| MCP | 提供无状态的 MCP <code>2026-07-28</code> Streamable HTTP 服务和工具资源 |
| OAuth | 内置 DCR、PKCE、资源指示器、旋转 Refresh Token 和 ChatGPT 兼容登录 |
| Provider | 支持 OpenAI 兼容接口、Anthropic、DeepSeek、MiMo、GLM、Kimi、Gemini、Qwen |
| Admin | 独立的 LAN/VPN 控制面，管理凭据、Provider、索引、记忆、任务和备份 |

## 系统架构

~~~text
Obsidian ── WebDAV ─────┐
                        │
ChatGPT/Agent ─ MCP ────┼── 数据面监听器 :8080
                        │
OAuth 客户端 ─ OAuth ───┘

管理员浏览器 ─────────────── Admin UI/API :8081
                              │
                              ▼
                         Application Services
                              │
                              ▼
                          Vault Core
                    ┌─────────┼─────────┐
                    ▼         ▼         ▼
                Vault 文件   SQLite   Outbox/Jobs
                              │         │
                              ▼         ▼
                         索引/记忆/Provider Worker
~~~

### 两个网络平面

- 数据面监听器提供 MCP、WebDAV、健康检查和公开 OAuth 元数据。对公网开放时
  必须使用 HTTPS，Admin 路由不会出现在这个监听器上。
- 控制面监听器提供 Admin UI/API 和首次初始化。默认绑定回环地址，发布到 LAN、
  VPN 或反向代理时由部署层负责限制来源；网络限制不能替代 Admin 密码。

### 数据边界

| 数据类别 | 存储位置 | 是否可重建 |
| --- | --- | --- |
| 用户笔记、附件、已提交语义记忆 | Vault 内容根目录 | 否，属于规范内容 |
| 凭据、配置、修订、任务、审计、OAuth 状态 | SQLite 运行状态 | 否，应纳入备份 |
| FTS、embedding、主题、记忆查询投影 | 派生状态 | 是，可从规范内容重建 |
| 修订历史内容 | 服务管理的 history 存储 | 否，按保留策略备份 |

复制 Vault 内容根目录后，得到的仍是普通 Obsidian Vault。服务的 SQLite、历史
和密钥不会写入这个内容根目录。

## 快速开始

### 环境要求

- Docker Engine 和 Docker Compose v2；
- 如果要从公网连接 ChatGPT，需要可公开访问的可信 HTTPS 域名；
- 如果要使用 Admin，需要本机、LAN 或 VPN 访问控制面监听器；
- 本地开发需要仓库锁定的 Rust 工具链和 Node.js/pnpm。

### 本地 Docker Compose

仓库根目录的 <code>compose.yaml</code> 用于本机开发和单机测试。它默认把数据面映射到
<code>8080</code>，把 Admin 映射到 <code>127.0.0.1:8081</code>，数据保存在 <code>./data</code>。

获取代码后进入仓库目录，再执行以下命令：

~~~bash
cd mcp-vault
docker compose config --quiet
docker compose up -d --build
curl --fail http://127.0.0.1:8080/health/ready
~~~

首次打开 <code>http://127.0.0.1:8081/</code>，按页面创建 Admin 用户。首次初始化只需要
用户名和密码，不需要 bootstrap token，也没有默认密码。

本机检查完成后，可使用 <code>docker compose down</code> 停止服务。不要删除 <code>./data</code>，
除非明确要重置测试实例。

### 使用已构建镜像

当前版本镜像可以这样构建：

~~~bash
docker build --platform linux/amd64 --tag mcp-vault:0.2.1 --tag mcp-vault:latest .
~~~

镜像包含 Rust 服务和编译后的 Admin 前端，运行用户为非 root 的 <code>mcpvault</code>，
入口命令为 <code>/usr/local/bin/mcp-vault</code>。在部署前可以执行：

~~~bash
docker run --rm --platform linux/amd64 --read-only --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m mcp-vault:0.2.1 --check-config
~~~

## 首次配置顺序

1. 在 Admin 页面创建第一个 Admin 用户。
2. 确认 Vault 内容根目录和扫描状态。
3. 创建独立的 WebDAV 凭据，复制一次性密码。
4. 在 Admin 的“MCP / ChatGPT OAuth”页面创建独立的 Vault OAuth 用户，或创建 PAT。
5. 如需语义检索，配置 embedding Provider，并按需绑定
   <code>embedding_note</code> 与 <code>embedding_memory</code>；索引页和记忆页可分别
   生成缺失向量，无需重新分析笔记或整理记忆。
6. 如需自动记忆，绑定 <code>memory_extraction</code> 模型，然后显式启用 Vault 级
   自动记忆；每篇来源笔记只需要一次模型调用。
7. 完成备份目录、保留策略和反向代理配置后，再将数据面发布到公网。

Admin 密码、WebDAV 密码、MCP PAT 和 Vault OAuth 密码属于不同安全平面，不能
相互替代。

## WebDAV 与 Obsidian

WebDAV 地址格式如下：

~~~text
https://vault.example.com/dav/v1/vaults/default/
~~~

使用 Admin 页面生成的 WebDAV 用户名和密码，不要把 Admin 密码或 MCP Token
填入 Obsidian。

服务支持 Obsidian 常用的 <code>OPTIONS</code>、<code>PROPFIND</code>、<code>GET</code>、<code>HEAD</code>、<code>PUT</code>、
<code>DELETE</code>、<code>MKCOL</code>、<code>COPY</code>、<code>MOVE</code>、<code>LOCK</code> 和 <code>UNLOCK</code>，并处理字节范围、ETag、条件请求和
目录深度限制。写入通过 Vault Core 完成，已知并发修改会返回冲突，不会静默覆盖。

服务会保留 <code>.obsidian/</code> 文件，但默认不把它们送入语义索引。客户端侧 WebDAV
加密会使服务无法读取和索引笔记内容；如果需要服务器侧搜索和 Agent 访问，
请关闭客户端加密。

NAS 或低版本内核部署必须确认 Vault 内容根目录支持安全的原子创建。服务优先
使用 <code>RENAME_NOREPLACE</code>；明确不支持该接口时，普通文件创建才会使用服务自己
创建的同目录临时文件硬链接兼容路径。这个路径不会提供用户可调用的硬链接
功能。文件或目录移动不会使用硬链接：服务会先用 Vault 级命名空间锁串行化
所有目标占用操作，再次确认目标不存在，然后以普通原子 <code>renameat</code> 兼容
同文件系统移动。跨文件系统移动仍会失败，不会隐式复制删除。

同一个 Vault 内容根目录只能由一个 MCP Vault 服务进程管理。直接在宿主机目录
中进行的并发写入不会参与进程内锁，应避免与 MCP/WebDAV 写入同时发生；非并发
的目录外变更仍会由 reconciliation 导入。

## MCP 接口

MCP 地址格式如下：

~~~text
https://vault.example.com/mcp/v1/vaults/default
~~~

服务目标协议版本为 MCP <code>2026-07-28</code>，通过官方 Rust SDK 协商兼容的旧版本。
协议边界保持无状态，每个请求都根据 endpoint 和 PAT/OAuth 凭据解析 Vault，
工具不会接受任意 <code>vault_id</code> 参数。

工具按权限过滤并保持稳定顺序：

~~~text
vault_overview       browse_index          recent_changes
search_notes         read_note             recall
get_memory           list_memories         create_note
edit_note            move_note             delete_note
note_history         restore_note_revision remember
update_memory        forget_memory
~~~

业务权限 scope 为：

~~~text
vault:discover  vault:read     vault:write   vault:delete
vault:history   memory:read    memory:write  memory:manage
~~~

MCP 结果使用结构化 JSON，并包含 <code>request_id</code>。工具错误不会泄露 SQL、绝对路径、
笔记正文、Provider 响应或凭据。

## ChatGPT OAuth

内置 OAuth 授权服务器位于数据面，支持：

- RFC 8414 授权服务器元数据；
- RFC 9728 受保护资源元数据；
- RFC 7591 公共客户端动态注册；
- Authorization Code + PKCE <code>S256</code>；
- RFC 8707 <code>resource</code> 和 RFC 9207 <code>iss</code>；
- 旋转 Refresh Token、本地撤销和 Vault/resource/client 绑定；
- 协议级 <code>offline_access</code>，不增加任何 Vault 权限。

公开端点如下：

~~~text
GET  /.well-known/oauth-authorization-server
POST /oauth/register
GET  /oauth/v2/authorize
POST /oauth/v2/authorize
POST /oauth/token
~~~

<code>/oauth/authorize</code> 和 <code>/oauth/v1/authorize</code> 仍作为兼容别名。反向代理必须把
整个 <code>/oauth/</code> 前缀转发到数据面，并为 OAuth 路径关闭 Nginx、CDN 和边缘缓存。

### ChatGPT 连接步骤

1. 设置 <code>MCP_VAULT_DATA_PUBLIC_ORIGIN</code>，值必须是实际的 HTTPS 公网 Origin。
2. 在 Admin 的“MCP / ChatGPT OAuth”页面创建独立的 Vault OAuth 用户和最大业务
   scope。
3. 在 ChatGPT 中只填写 MCP endpoint，不要手工填写 callback、client ID、secret
   或 token。
4. ChatGPT 会执行 DCR，打开 MCP Vault 登录页，并完成 PKCE 授权码交换。
5. 登录页中的“保持长期连接”对应 <code>offline_access</code>，它不是文件或记忆权限。

Access Token 有效期为 1 小时。每次成功刷新都会重新获得 180 天的 Refresh Token
空闲期限。相同旧 Refresh Token 在 60 秒内的重复提交只返回 <code>invalid_grant</code>，
不会撤销第一次成功签发的新令牌；超过宽限期的重放才会撤销整个令牌 family。

从 <code>0.1.14</code> 或 <code>0.1.15</code> 升级到 <code>0.1.16</code>
不会主动使已有未过期 Token 失效，旧 grant 也可以继续刷新。
如果希望 ChatGPT 保存新的 <code>offline_access</code> grant，升级后建议在 ChatGPT 中点一次
“重新连接”。Token 已过期、被撤销或被 ChatGPT 丢失时，才必须重新登录。

包含迁移 0015 的 v2.1 版本不会自动删除旧记忆、OAuth 或 Vault 内容。升级后旧记忆
引擎立即退出所有模型读取入口；显式保留与笔记重新生成必须通过 Admin 的预检和明确
迁移确认完成。升级前仍应创建并验证备份；数据库迁移为前向迁移。

## 长期记忆

长期记忆只有“当前”状态，并分为两种归属：

- 显式记忆：Agent 或 Admin 明确保存，每条拥有独立的规范 Markdown；
- 笔记派生记忆：每篇来源笔记按稳定 File ID 拥有一组完整的当前记忆。

自动记忆是显式启用的事件驱动功能。只需绑定
<code>memory_extraction</code> 模型；普通 Markdown 不需要特殊标签、目录或
frontmatter。每篇笔记只调用一次模型：

~~~text
读取当前笔记、File ID 与内容哈希
    ↓
模型返回完整的 {"memories":[...]} 内容集合
    ↓
本地校验并持久化可恢复快照
    ↓
通过 Vault Core 原子替换该来源的规范 memory-set
    ↓
替换当前投影、FTS，并按需生成向量
~~~

模型只建议 <code>content</code> 与可选 <code>kind/tags</code>；ID、路径、来源、修订、
替换和删除全部由服务端控制。重要技术知识可以被提炼，但教程、第三人称事实或
未采纳建议不能被改写成用户已经掌握或采用的事实。

当前规范文件位于受管命名空间：

~~~text
_mcp-vault/memory/current/explicit/{memory_id}.md
_mcp-vault/memory/current/sources/{source_file_id}.md
~~~

来源内容哈希一旦变化，旧派生集合会立即退出所有记忆读取入口；新提取失败时也
不会重新开放旧内容。相同 File ID、相同内容的移动只更新展示路径，不调用模型。
删除来源会删除其派生集合；同一路径删除后重建得到新 File ID，不继承旧集合。

<code>forget_memory</code> 始终删除当前记忆，不再归档。删除笔记派生条目会重写所属
集合，并暂停该来源的自动提取，直到 Admin 以当前 set revision 明确恢复。显式
记忆通过 <code>remember</code> 直接写入，不经过提取或全局整理。

<code>recall</code> 只读取当前、来源哈希仍有效的本地投影，不在查询时调用生成式
模型。词法/实体证据始终可用；语义单独命中只有在对应 embedding profile 已用
真实标注集校准后才可进入普通召回。返回的 <code>score</code> 是融合排序分，原始
余弦仅在诊断字段 <code>semantic_cosine</code> 中出现。

升级到 v2.1 时，迁移 0015 只新增 current-only 表。旧生命周期、Phase 1/2、
source-health 与别名表不会自动删除，也不会再被 MCP、Admin 记忆读取、resource、
recall 或 embedding source 使用。Admin 必须先运行无正文的迁移预检，再用
<code>MIGRATE_MEMORY_V2_1</code> 明确确认：可可靠识别的显式记忆保留原 ID，笔记
派生内容从当前来源重新生成，混合/不明归属只报告、不猜测。生产升级前仍应创建并
验证备份。

## 文件、数据库和备份

容器默认数据目录为 <code>/data</code>：

~~~text
/data/vaults       Vault 内容根目录
/data/state        SQLite 和运行状态
/data/history      修订历史内容
/data/secrets      安装密钥（默认 /data/secrets/master-key）
/data/backups      已验证的备份归档
/data/models       可选的本地 embedding 模型缓存
~~~

应当一起备份 <code>/data/vaults</code>、<code>/data/state</code> 和 <code>/data/history</code>。安装主密钥默认
不包含在普通可下载备份中，必须单独保管；丢失已有密钥时，服务不会自动生成
新密钥覆盖加密状态。

备份和恢复由 Admin 控制面执行。恢复前会校验归档路径、大小、校验和、SQLite
完整性和密钥版本。不要直接编辑 SQLite，也不要手动删除 <code>_mcp-vault/memory/</code>
中的受管文件。

## 关键配置

| 环境变量 | 作用 |
| --- | --- |
| <code>MCP_VAULT_DATA_DIR</code> | 服务数据根目录，默认 <code>/data</code>（本地默认 <code>./data</code>） |
| <code>MCP_VAULT_DATABASE_URL</code> | SQLite 地址；默认位于数据根目录的 state 下 |
| <code>MCP_VAULT_SECRETS_DIR</code> | 安装主密钥目录 |
| <code>MCP_VAULT_MASTER_KEY_FILE</code> | 可选的外部主密钥文件 |
| <code>MCP_VAULT_DATA_BIND</code> | 数据面监听地址，默认 <code>0.0.0.0:8080</code> |
| <code>MCP_VAULT_ADMIN_BIND</code> | 控制面监听地址，默认 <code>127.0.0.1:8081</code> |
| <code>MCP_VAULT_ADMIN_PUBLISH</code> | Compose 发布 Admin 的主机地址和端口 |
| <code>MCP_VAULT_DATA_HOSTS</code> | 数据面允许的精确 Host authority 列表 |
| <code>MCP_VAULT_DATA_ORIGINS</code> | 数据面允许的浏览器 Origin 列表 |
| <code>MCP_VAULT_DATA_PUBLIC_ORIGIN</code> | 对外公布的规范 HTTPS Origin，生产 OAuth 必填 |
| <code>MCP_VAULT_ADMIN_ORIGINS</code> | Admin 允许的精确浏览器 Origin 列表 |
| <code>MCP_VAULT_BACKUP_DIR</code> | 备份归档目录 |
| <code>MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS</code> | 文件系统对账间隔，默认 300 秒 |
| <code>MCP_VAULT_METRICS_ENABLED</code> | 是否启用非敏感 Prometheus 指标 |
| <code>MCP_VAULT_LOG_FORMAT</code> | <code>json</code> 或本地开发用的可读格式 |

<code>MCP_VAULT_ADMIN_ALLOWED_CIDRS</code>、<code>MCP_VAULT_BOOTSTRAP_TOKEN</code> 和
<code>MCP_VAULT_BOOTSTRAP_TOKEN_FILE</code> 已废弃。Admin 来源限制属于 Compose、主机防火墙、
VPN 或反向代理策略，服务不会静默接受这些旧配置。

## 反向代理要求

公网虚拟主机只应转发以下数据面路径：

~~~text
/mcp/
/dav/
/health/
/.well-known/oauth-protected-resource
/.well-known/oauth-authorization-server
/oauth/
~~~

必须保留非标准 WebDAV 方法、<code>Authorization</code>、MCP 请求头和流式响应，并设置
<code>X-Forwarded-Proto: https</code>。不要把 <code>/api/</code>、<code>/setup</code> 或 Admin 静态资源转发到
公网虚拟主机。

详细的 Nginx HTTPS 示例位于 [deploy/nginx-https/](deploy/nginx-https/README.md)，
已有 Nginx 的单服务示例位于 [deploy/existing-nginx/](deploy/existing-nginx/README.md)。

## 开发与验证

仓库使用 Rust <code>1.94.0</code>、官方 <code>rmcp</code>、SQLx、Axum、React、TypeScript、Vite 和
pnpm。常用命令如下：

~~~bash
make fmt-check
make lint
make test
make frontend-lint
make frontend-test
make frontend-build
make docs-check
make e2e
make conformance
make migration-check
~~~

其中：

- <code>make e2e</code> 运行真实 HTTP 的 OAuth、MCP 和 WebDAV smoke；
- <code>make conformance</code> 运行固定版本的官方 MCP conformance 场景；
- <code>make migration-check</code> 检查历史数据库升级和迁移完整性；
- <code>make docs-check</code> 检查文档、工作区结构和 Rust API 文档。

发布前还应执行 WebDAV Litmus、备份恢复、双 Vault 隔离、Provider 故障和目标
Obsidian 插件版本的手工验证。自动化通过不等于公网 DNS、TLS、CDN 或 ChatGPT
账号路径已经完成验收。

## 文档导航

- [文档地图](docs/README.md)
- [产品需求](docs/product-requirements.md)
- [系统架构](docs/architecture.md)
- [接口契约](docs/interfaces.md)
- [数据模型](docs/data-model.md)
- [记忆系统](docs/memory-system.md)
- [安全设计](docs/security.md)
- [Admin 与配置](docs/admin-and-configuration.md)
- [部署与运维](docs/deployment-and-operations.md)
- [兼容性矩阵](docs/compatibility-matrix.md)
- [开发与测试](docs/development-and-testing.md)
- [发布就绪清单](docs/release-readiness.md)
- [架构决策记录](docs/adr/README.md)

## 许可证

Rust workspace 元数据声明许可证为 Apache-2.0。对外发布时应同时提供对应的
许可证文本和第三方依赖声明。
