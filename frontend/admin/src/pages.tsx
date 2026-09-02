import { useEffect, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';

import { adminApi } from './api';
import {
  CopyField,
  EmptyState,
  InfoGrid,
  InfoItem,
  Metric,
  Notice,
  PasswordPolicyHelp,
  type NoticeTone,
  Panel,
  RawData,
  SecretReveal,
  StatusBadge,
} from './ui';
import {
  type JsonObject,
  type Page,
  arrayRecords,
  asRecord,
  booleanValue,
  formatBytes,
  formatPercent,
  formatRequestError,
  formatTime,
  jobErrorLabel,
  memoryTypeLabel,
  numberValue,
  statusLabel,
  statusTone,
  stringValue,
  truncateId,
} from './view-model';

type Notify = (message: string, tone?: NoticeTone) => void;

export function Dashboard({ data, onNavigate }: { data: JsonObject | null; onNavigate: (page: Page) => void }) {
  const vault = asRecord(data?.vault);
  const files = asRecord(data?.files);
  const index = asRecord(data?.index);
  const memory = asRecord(data?.memory);
  const jobs = asRecord(data?.jobs);
  const providers = arrayRecords(data?.providers);
  const ready = booleanValue(data?.ready);

  return (
    <div className="page-stack">
      <section className="hero-card">
        <div>
          <StatusBadge tone={ready ? 'success' : 'warning'}>{ready ? '服务已就绪' : '服务正在准备'}</StatusBadge>
          <h2>{vaultName(vault.name)}</h2>
          <p className="path-text">{stringValue(vault.content_root, '完成初始化后会显示 Vault 路径')}</p>
        </div>
        <button className="primary-button" type="button" onClick={() => onNavigate('webdav')}>
          连接 Obsidian
        </button>
      </section>

      <section className="metric-grid" aria-label="Vault 关键指标">
        <Metric
          label="Markdown 笔记"
          value={numberValue(files.notes)}
          detail={`共跟踪 ${numberValue(files.entries)} 个条目`}
        />
        <Metric
          label="长期记忆"
          value={numberValue(memory.active)}
          detail={`${numberValue(memory.pending_consolidation)} 条原始输入等待整理`}
        />
        <Metric label="后台任务" value={numberValue(jobs.pending)} detail="等待或正在执行" />
        <Metric label="索引覆盖率" value={formatPercent(index.coverage_ratio)} detail={`${numberValue(index.indexed_notes)} / ${numberValue(index.total_notes)} 篇已索引`} />
      </section>

      <section className="panel-grid">
        <Panel title="快速开始" eyebrow="推荐顺序" description="按下面三步即可完成常用接入。">
          <div className="quick-actions">
            <QuickAction number="1" title="连接 Obsidian" detail="创建 WebDAV 设备凭据" onClick={() => onNavigate('webdav')} />
            <QuickAction number="2" title="连接 Agent" detail="创建最小权限 MCP PAT" onClick={() => onNavigate('mcp')} />
            <QuickAction number="3" title="按需启用 AI" detail="核心同步和搜索不依赖模型" onClick={() => onNavigate('providers')} />
          </div>
        </Panel>

        <Panel title="运行状态" eyebrow="安全降级" description="AI 服务异常不会阻塞文件同步和全文搜索。">
          <div className="summary-list">
            <SummaryRow label="Vault 状态" value={<StatusBadge tone={statusTone(vault.status)}>{statusLabel(vault.status)}</StatusBadge>} />
            <SummaryRow label="附件数量" value={numberValue(files.attachments)} />
            <SummaryRow label="AI 服务记录" value={providers.length} />
            <SummaryRow label="当前版本" value={stringValue(data?.version)} mono />
          </div>
        </Panel>
      </section>
    </div>
  );
}

function QuickAction({ number, title, detail, onClick }: { number: string; title: string; detail: string; onClick: () => void }) {
  return (
    <button className="quick-action" type="button" onClick={onClick}>
      <span>{number}</span>
      <div>
        <strong>{title}</strong>
        <small>{detail}</small>
      </div>
      <b aria-hidden="true">→</b>
    </button>
  );
}

export function ManagementPage({ page, data, onRefresh }: { page: Page; data: JsonObject | null; onRefresh: () => void }) {
  const [message, setMessage] = useState<{ text: string; tone: NoticeTone } | null>(null);
  const [oneTimeSecret, setOneTimeSecret] = useState<string | null>(null);

  useEffect(() => {
    setMessage(null);
    setOneTimeSecret(null);
  }, [page]);

  const notify: Notify = (text, tone = 'success') => setMessage({ text, tone });
  let content: ReactNode;

  switch (page) {
    case 'vault':
      content = <VaultPage data={data} notify={notify} onRefresh={onRefresh} />;
      break;
    case 'webdav':
      content = <WebDavPage data={data} notify={notify} onSecret={setOneTimeSecret} onRefresh={onRefresh} />;
      break;
    case 'mcp':
      content = <McpPage data={data} notify={notify} onSecret={setOneTimeSecret} onRefresh={onRefresh} />;
      break;
    case 'providers':
      content = <ProviderPage data={data} notify={notify} onRefresh={onRefresh} />;
      break;
    case 'index':
      content = <IndexPage data={data} notify={notify} onRefresh={onRefresh} />;
      break;
    case 'memory':
      content = <MemoryPage data={data} notify={notify} onRefresh={onRefresh} />;
      break;
    case 'jobs':
      content = <JobsPage data={data} notify={notify} onRefresh={onRefresh} />;
      break;
    case 'audit':
      content = <AuditPage data={data} />;
      break;
    case 'backup':
      content = <BackupPage data={data} notify={notify} onRefresh={onRefresh} />;
      break;
    case 'system':
      content = <SystemPage data={data} />;
      break;
    default:
      content = null;
  }

  return (
    <div className="page-stack">
      {message ? <Notice tone={message.tone}>{message.text}</Notice> : null}
      {oneTimeSecret ? <SecretReveal secret={oneTimeSecret} onDismiss={() => setOneTimeSecret(null)} /> : null}
      {content}
      <RawData data={data} />
    </div>
  );
}

function VaultPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const vault = asRecord(data);
  const [busy, setBusy] = useState(false);
  const availability = stringValue(vault.availability, stringValue(vault.status, 'unknown'));

  async function rescan() {
    setBusy(true);
    try {
      await adminApi.request('/vault/rescan', { method: 'POST' });
      notify('重新扫描任务已加入后台队列。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  async function retryInitialization() {
    setBusy(true);
    try {
      await adminApi.request('/vault/initialization/retry', { method: 'POST' });
      notify('Vault 初始化任务已重新加入后台队列。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel
      title={vaultName(vault.name)}
      eyebrow="当前知识库"
      description="内容根目录是普通 Obsidian Vault，浏览器不能直接修改路径。"
      actions={<StatusBadge tone={statusTone(availability)}>{statusLabel(availability)}</StatusBadge>}
    >
      {availability === 'initializing' ? <Notice tone="info">首次扫描、索引和记忆状态正在初始化；完成前该 Vault 的 WebDAV 和 MCP 链接暂不可用。</Notice> : null}
      {availability === 'error' ? <Notice tone="danger">该 Vault 初始化或恢复失败，其他 Vault 不受影响。检查任务详情后可重试初始化。</Notice> : null}
      <InfoGrid>
        <InfoItem label="标识" value={stringValue(vault.slug)} mono />
        <InfoItem label="内容目录" value={stringValue(vault.content_root)} mono />
        <InfoItem label="内部目录" value={stringValue(vault.reserved_root)} mono />
        <InfoItem label="设置版本" value={numberValue(vault.settings_revision)} />
      </InfoGrid>
      <div className="button-row section-actions">
        <button className="secondary-button" type="button" disabled={busy} onClick={() => void rescan()}>
          {busy ? '正在提交…' : '重新扫描 Vault'}
        </button>
        {availability === 'error' ? (
          <button className="secondary-button" type="button" disabled={busy} onClick={() => void retryInitialization()}>
            重试初始化
          </button>
        ) : null}
      </div>
      <details className="disclosure" open>
        <summary>编辑基本设置</summary>
        <VaultForm data={vault} notify={notify} onRefresh={onRefresh} />
      </details>
    </Panel>
  );
}

function VaultForm({ data, notify, onRefresh }: { data: JsonObject; notify: Notify; onRefresh: () => void }) {
  const [name, setName] = useState(vaultName(data.name));
  const [status, setStatus] = useState(stringValue(data.status, 'active'));
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setName(vaultName(data.name));
    setStatus(stringValue(data.status, 'active'));
  }, [data.name, data.status]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    try {
      await adminApi.request('/vault', {
        method: 'PATCH',
        body: {
          name,
          status,
          expected_settings_revision:
            typeof data.settings_revision === 'number' ? data.settings_revision : undefined,
        },
      });
      notify('Vault 设置已保存。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="compact-form" onSubmit={submit}>
      <div className="form-grid">
        <label>
          显示名称
          <input required value={name} onChange={(event) => setName(event.target.value)} />
        </label>
        <label>
          运行状态
          <select value={status} onChange={(event) => setStatus(event.target.value)}>
            <option value="active">正常</option>
            <option value="maintenance">维护中</option>
            <option value="disabled">已停用</option>
          </select>
        </label>
      </div>
      <button className="primary-button" disabled={busy} type="submit">
        {busy ? '正在保存…' : '保存设置'}
      </button>
    </form>
  );
}

function WebDavPage({
  data,
  notify,
  onSecret,
  onRefresh,
}: {
  data: JsonObject | null;
  notify: Notify;
  onSecret: (secret: string) => void;
  onRefresh: () => void;
}) {
  const credentials = arrayRecords(data?.credentials);

  async function revoke(credential: JsonObject) {
    const name = stringValue(credential.name, '此设备');
    if (!window.confirm(`确定撤销“${name}”的 WebDAV 凭据吗？该设备会立即停止同步。`)) return;
    try {
      await adminApi.request(`/webdav/credentials/${stringValue(credential.id, '')}`, { method: 'DELETE' });
      notify('WebDAV 凭据已撤销。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    }
  }

  return (
    <Panel title="已连接设备" eyebrow="Obsidian / WebDAV" description="每台设备使用独立凭据，便于单独撤销。">
      <div className="copy-stack connection-block">
        <CopyField label="WebDAV 地址" value={stringValue(data?.webdav_endpoint)} />
      </div>
      {credentials.length === 0 ? (
        <EmptyState title="还没有设备凭据" detail="展开下面的表单，为第一台 Obsidian 设备创建凭据。" />
      ) : (
        <div className="record-list">
          {credentials.map((credential) => {
            const revoked = typeof credential.revoked_at === 'number';
            const permissions = Array.isArray(credential.permissions) ? credential.permissions.join('、') : '—';
            return (
              <article className="record-item" key={stringValue(credential.id)}>
                <div className="record-main">
                  <div className="record-title">
                    <strong>{stringValue(credential.name, '未命名设备')}</strong>
                    <StatusBadge tone={revoked ? 'neutral' : 'success'}>{revoked ? '已撤销' : '可用'}</StatusBadge>
                  </div>
                  <p>用户名：<code>{stringValue(credential.username)}</code></p>
                  <small>权限：{permissions} · 最近使用：{formatTime(credential.last_used_at)}</small>
                </div>
                {!revoked ? (
                  <button className="danger-link" type="button" onClick={() => void revoke(credential)}>
                    撤销
                  </button>
                ) : null}
              </article>
            );
          })}
        </div>
      )}
      <details className="disclosure" open={credentials.length === 0}>
        <summary>添加一台 Obsidian 设备</summary>
        <WebDavForm notify={notify} onSecret={onSecret} onRefresh={onRefresh} />
      </details>
    </Panel>
  );
}

function WebDavForm({ notify, onSecret, onRefresh }: { notify: Notify; onSecret: (secret: string) => void; onRefresh: () => void }) {
  const [values, setValues] = useState({ name: '', username: '', password: '' });
  const [permissions, setPermissions] = useState(['read', 'write', 'delete']);
  const [busy, setBusy] = useState(false);

  function togglePermission(permission: string) {
    setPermissions((current) =>
      current.includes(permission) ? current.filter((value) => value !== permission) : [...current, permission],
    );
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (permissions.length === 0) {
      notify('请至少选择一个 WebDAV 权限。', 'warning');
      return;
    }
    setBusy(true);
    try {
      const result = await adminApi.request<{ password: string }>('/webdav/credentials', {
        method: 'POST',
        body: { ...values, permissions },
      });
      onSecret(result.password);
      notify('设备凭据已创建，请立即复制密码。');
      setValues({ name: '', username: '', password: '' });
      setPermissions(['read', 'write', 'delete']);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="compact-form" onSubmit={submit}>
      <div className="form-grid">
        <label>
          设备名称
          <input required value={values.name} onChange={(event) => setValues({ ...values, name: event.target.value })} placeholder="例如：MacBook 上的 Obsidian" />
        </label>
        <label>
          WebDAV 用户名
          <input required value={values.username} onChange={(event) => setValues({ ...values, username: event.target.value })} placeholder="例如：obsidian-mac" />
        </label>
        <label className="form-span">
          WebDAV 密码
          <input required type="password" value={values.password} onChange={(event) => setValues({ ...values, password: event.target.value })} autoComplete="new-password" aria-describedby="webdav-password-policy" placeholder="为这台设备设置独立密码" />
          <PasswordPolicyHelp id="webdav-password-policy" />
        </label>
      </div>
      <fieldset className="choice-group">
        <legend>允许的同步操作</legend>
        <Choice checked={permissions.includes('read')} label="读取" detail="下载笔记和附件" onChange={() => togglePermission('read')} />
        <Choice checked={permissions.includes('write')} label="写入" detail="上传和更新文件" onChange={() => togglePermission('write')} />
        <Choice checked={permissions.includes('delete')} label="删除" detail="同步本地删除操作" onChange={() => togglePermission('delete')} />
      </fieldset>
      <Notice tone="info">完整双向同步通常需要读取、写入和删除三个权限。</Notice>
      <button className="primary-button" disabled={busy} type="submit">
        {busy ? '正在创建…' : '创建凭据并显示一次'}
      </button>
    </form>
  );
}

const patScopeOptions = [
  { value: 'vault:discover', label: '发现知识库', detail: '查看概览、主题和最近变化' },
  { value: 'vault:read', label: '读取笔记', detail: '搜索和读取原始内容' },
  { value: 'vault:write', label: '编辑笔记', detail: '创建和修改笔记' },
  { value: 'vault:delete', label: '删除笔记', detail: '允许删除文件，谨慎开启' },
  { value: 'vault:history', label: '历史版本', detail: '查看和恢复历史版本' },
  { value: 'memory:read', label: '读取记忆', detail: '召回长期上下文' },
  { value: 'memory:write', label: '写入记忆', detail: '创建显式长期记忆' },
  { value: 'memory:manage', label: '管理记忆', detail: '归档、合并或删除记忆' },
];

const patPresets = [
  {
    id: 'read-only',
    label: '只读助手',
    detail: '浏览、搜索、读取和召回记忆',
    scopes: ['vault:discover', 'vault:read', 'memory:read'],
    recommended: true,
  },
  {
    id: 'editor',
    label: '可编辑助手',
    detail: '额外允许编辑笔记和写入记忆',
    scopes: ['vault:discover', 'vault:read', 'vault:write', 'vault:history', 'memory:read', 'memory:write'],
  },
  {
    id: 'manager',
    label: '完全管理',
    detail: '包含删除与记忆管理，请谨慎使用',
    scopes: patScopeOptions.map((option) => option.value),
  },
];

function McpPage({
  data,
  notify,
  onSecret,
  onRefresh,
}: {
  data: JsonObject | null;
  notify: Notify;
  onSecret: (secret: string) => void;
  onRefresh: () => void;
}) {
  const tokens = arrayRecords(data?.tokens);
  const [externalOauthOpen, setExternalOauthOpen] = useState(false);
  const localOAuth = asRecord(data?.local_oauth);
  const localOAuthUser = asRecord(localOAuth.user);
  const localOAuthConfigured = booleanValue(localOAuth.configured) && booleanValue(localOAuthUser.enabled);

  async function revoke(token: JsonObject) {
    const name = stringValue(token.name, '此 Token');
    if (!window.confirm(`确定撤销“${name}”吗？使用它的 Agent 会立即失去访问权限。`)) return;
    try {
      await adminApi.request(`/mcp/tokens/${stringValue(token.id, '')}`, { method: 'DELETE' });
      notify('MCP PAT 已撤销。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    }
  }

  return (
    <div className="page-stack">
      <Panel title="连接信息" eyebrow="MCP 接入地址" description="在 Agent 客户端中填写下面的地址和凭据。">
        <div className="copy-stack">
          <CopyField label="MCP 地址" value={stringValue(data?.mcp_endpoint)} />
          <CopyField label="OAuth 资源元数据" value={stringValue(data?.oauth_protected_resource_metadata_url)} />
          <CopyField label="内置 OAuth 服务元数据" value={stringValue(data?.oauth_authorization_server_metadata_url)} />
          <CopyField label="WebDAV 地址" value={stringValue(data?.webdav_endpoint)} />
        </div>
        <p className="muted compact-text">支持协议版本：{Array.isArray(data?.supported_mcp_revisions) ? data.supported_mcp_revisions.join('、') : '—'}</p>
      </Panel>

      <Panel
        title="ChatGPT OAuth"
        eyebrow="内置授权服务器"
        description="直接使用 MCP Vault 登录，不需要部署外部 OAuth 服务。"
        actions={<StatusBadge tone={localOAuthConfigured ? 'success' : 'warning'}>{localOAuthConfigured ? '已启用' : '待配置'}</StatusBadge>}
      >
        <Notice tone="info">
          在 ChatGPT 中添加远程 MCP 时只填写上面的 MCP 地址。ChatGPT 会自动发现本站 OAuth、注册公开客户端并跳转到 MCP Vault 登录页。
        </Notice>
        {localOAuthConfigured ? (
          <div className="summary-list">
            <SummaryRow label="OAuth 用户名" value={stringValue(localOAuthUser.username)} mono />
            <SummaryRow label="授权范围" value={Array.isArray(localOAuthUser.scopes) ? localOAuthUser.scopes.join(' · ') : '—'} />
            <SummaryRow label="密码更新时间" value={formatTime(localOAuthUser.password_changed_at)} />
          </div>
        ) : (
          <EmptyState title="还没有内置 OAuth 登录" detail="创建一个与 Admin 完全独立的 Vault OAuth 用户后，即可连接 ChatGPT。" />
        )}
        <details className="disclosure" open={!localOAuthConfigured}>
          <summary>{localOAuthConfigured ? '轮换 OAuth 登录与权限' : '创建 Vault OAuth 登录'}</summary>
          <LocalOAuthForm configured={localOAuthConfigured} notify={notify} onRefresh={onRefresh} />
        </details>
      </Panel>

      <Panel title="个人访问 Token" eyebrow="PAT" description="每个 Agent 使用独立 Token，并只授予所需权限。">
        {tokens.length === 0 ? (
          <EmptyState title="还没有 MCP Token" detail="展开下面的表单创建一个最小权限 Token。" />
        ) : (
          <div className="record-list">
            {tokens.map((token) => {
              const revoked = typeof token.revoked_at === 'number';
              return (
                <article className="record-item" key={stringValue(token.id)}>
                  <div className="record-main">
                    <div className="record-title">
                      <strong>{stringValue(token.name, '未命名 Agent')}</strong>
                      <StatusBadge tone={revoked ? 'neutral' : 'success'}>{revoked ? '已撤销' : '可用'}</StatusBadge>
                    </div>
                    <p>前缀：<code>{stringValue(token.token_prefix)}</code></p>
                    <small>{Array.isArray(token.scopes) ? token.scopes.join(' · ') : '—'} · 最近使用：{formatTime(token.last_used_at)}</small>
                  </div>
                  {!revoked ? <button className="danger-link" type="button" onClick={() => void revoke(token)}>撤销</button> : null}
                </article>
              );
            })}
          </div>
        )}
        <details className="disclosure" open={tokens.length === 0}>
          <summary>创建 MCP PAT</summary>
          <McpTokenForm notify={notify} onSecret={onSecret} onRefresh={onRefresh} />
        </details>
      </Panel>

      <details className="advanced-section" onToggle={(event) => setExternalOauthOpen(event.currentTarget.open)}>
        <summary>
          <span>高级：外部 OAuth/OIDC 兼容</span>
          <small>仅供已有身份提供商的部署使用</small>
        </summary>
        {externalOauthOpen ? (
          <OAuthForms
            mcpEndpoint={stringValue(data?.mcp_endpoint)}
            metadataUrl={stringValue(data?.oauth_protected_resource_metadata_url)}
            notify={notify}
          />
        ) : null}
      </details>
    </div>
  );
}

function LocalOAuthForm({ configured, notify, onRefresh }: { configured: boolean; notify: Notify; onRefresh: () => void }) {
  const [username, setUsername] = useState('chatgpt');
  const [password, setPassword] = useState('');
  const [scopes, setScopes] = useState(['vault:discover', 'vault:read', 'memory:read']);
  const [busy, setBusy] = useState(false);

  function toggleScope(scope: string) {
    setScopes((current) => (current.includes(scope) ? current.filter((value) => value !== scope) : [...current, scope]));
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (scopes.length === 0) {
      notify('请至少选择一个 OAuth 权限。', 'warning');
      return;
    }
    setBusy(true);
    try {
      await adminApi.request('/mcp/oauth/local', {
        method: 'PUT',
        body: { username, password, scopes },
      });
      setPassword('');
      notify(configured ? 'OAuth 登录已轮换，旧授权和令牌已全部撤销。' : '内置 OAuth 已启用，可以连接 ChatGPT。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    if (!window.confirm('确定停用内置 OAuth 吗？所有 ChatGPT OAuth 授权会立即失效。')) return;
    setBusy(true);
    try {
      await adminApi.request('/mcp/oauth/local', { method: 'DELETE' });
      notify('内置 OAuth 已停用，已有授权和令牌已撤销。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="compact-form" onSubmit={submit}>
      <div className="form-grid">
        <label>
          Vault OAuth 用户名
          <input required autoComplete="username" value={username} onChange={(event) => setUsername(event.target.value)} />
        </label>
        <label>
          独立 OAuth 密码
          <input required type="password" autoComplete="new-password" aria-describedby="oauth-password-policy" value={password} onChange={(event) => setPassword(event.target.value)} />
        </label>
      </div>
      <PasswordPolicyHelp id="oauth-password-policy" />
      <fieldset className="preset-group">
        <legend>选择授权模板</legend>
        {patPresets.map((preset) => {
          const selected = sameValues(scopes, preset.scopes);
          return (
            <button className={`preset-card${selected ? ' preset-card--selected' : ''}`} key={preset.id} type="button" aria-pressed={selected} onClick={() => setScopes([...preset.scopes])}>
              <span>{preset.label}{preset.recommended ? <small>推荐</small> : null}</span>
              <p>{preset.detail}</p>
            </button>
          );
        })}
      </fieldset>
      <details className="disclosure permission-disclosure">
        <summary>自定义权限（已选 {scopes.length} 项）</summary>
        <fieldset className="choice-group choice-group--two-columns">
          <legend className="visually-hidden">逐项选择 OAuth Scope</legend>
          {patScopeOptions.map((option) => (
            <Choice key={option.value} checked={scopes.includes(option.value)} label={option.label} detail={option.detail} technical={option.value} onChange={() => toggleScope(option.value)} />
          ))}
        </fieldset>
      </details>
      <Notice tone="warning">此密码只用于公网 OAuth 登录，不是 Admin 密码。每次保存都会撤销旧授权，适合安全轮换。</Notice>
      <div className="button-row">
        <button className="primary-button" disabled={busy} type="submit">{busy ? '正在保存…' : configured ? '轮换登录并撤销旧令牌' : '启用内置 OAuth'}</button>
        {configured ? <button className="danger-link" disabled={busy} type="button" onClick={() => void disable()}>停用内置 OAuth</button> : null}
      </div>
    </form>
  );
}

function McpTokenForm({ notify, onSecret, onRefresh }: { notify: Notify; onSecret: (secret: string) => void; onRefresh: () => void }) {
  const [name, setName] = useState('');
  const [scopes, setScopes] = useState(['vault:discover', 'vault:read', 'memory:read']);
  const [busy, setBusy] = useState(false);

  function toggleScope(scope: string) {
    setScopes((current) => (current.includes(scope) ? current.filter((value) => value !== scope) : [...current, scope]));
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (scopes.length === 0) {
      notify('请至少选择一个 MCP 权限。', 'warning');
      return;
    }
    setBusy(true);
    try {
      const result = await adminApi.request<{ secret: string }>('/mcp/tokens', {
        method: 'POST',
        body: { name, scopes },
      });
      onSecret(result.secret);
      notify('MCP PAT 已创建，请立即复制。');
      setName('');
      setScopes(['vault:discover', 'vault:read', 'memory:read']);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="compact-form" onSubmit={submit}>
      <label>
        Agent 名称
        <input required value={name} onChange={(event) => setName(event.target.value)} placeholder="例如：个人知识助手" />
      </label>
      <fieldset className="preset-group">
        <legend>选择权限模板</legend>
        {patPresets.map((preset) => {
          const selected = sameValues(scopes, preset.scopes);
          return (
            <button
              className={`preset-card${selected ? ' preset-card--selected' : ''}`}
              key={preset.id}
              type="button"
              aria-pressed={selected}
              onClick={() => setScopes([...preset.scopes])}
            >
              <span>{preset.label}{preset.recommended ? <small>推荐</small> : null}</span>
              <p>{preset.detail}</p>
            </button>
          );
        })}
      </fieldset>
      <details className="disclosure permission-disclosure">
        <summary>自定义权限（已选 {scopes.length} 项）</summary>
        <fieldset className="choice-group choice-group--two-columns">
          <legend className="visually-hidden">逐项选择 MCP Scope</legend>
          {patScopeOptions.map((option) => (
            <Choice key={option.value} checked={scopes.includes(option.value)} label={option.label} detail={option.detail} technical={option.value} onChange={() => toggleScope(option.value)} />
          ))}
        </fieldset>
      </details>
      <Notice tone="info">“只读助手”适合大多数个人 Agent；需要写入时再切换模板或自定义。</Notice>
      <button className="primary-button" disabled={busy} type="submit">
        {busy ? '正在创建…' : '创建 PAT 并显示一次'}
      </button>
    </form>
  );
}

function OAuthForms({
  mcpEndpoint,
  metadataUrl,
  notify,
}: {
  mcpEndpoint: string;
  metadataUrl: string;
  notify: Notify;
}) {
  const [issuerValues, setIssuerValues] = useState({
    name: '',
    issuer_url: '',
    audience: mcpEndpoint,
    resource: mcpEndpoint,
    jwks_cache_json: '',
  });
  const [grantValues, setGrantValues] = useState({ issuer_id: '', subject: '', scopes: 'vault:discover,vault:read' });
  const [issuers, setIssuers] = useState<JsonObject[]>([]);
  const [grants, setGrants] = useState<JsonObject[]>([]);
  const [busy, setBusy] = useState(false);

  async function refreshOAuth() {
    const [issuerData, grantData] = await Promise.all([
      adminApi.request<{ issuers: unknown[] }>('/mcp/oauth'),
      adminApi.request<{ grants: unknown[] }>('/mcp/oauth/grants'),
    ]);
    const nextIssuers = issuerData.issuers.map(asRecord);
    setIssuers(nextIssuers);
    setGrants(grantData.grants.map(asRecord));
    setGrantValues((current) => ({ ...current, issuer_id: current.issuer_id || stringValue(nextIssuers[0]?.id, '') }));
  }

  useEffect(() => {
    void refreshOAuth().catch((error: unknown) => notify(formatRequestError(error), 'danger'));
    // Advanced controls load only after the operator expands this section.
  }, []);

  return (
    <div className="advanced-content">
      <Notice tone="info">
        仅在已有外部身份提供商时使用。ChatGPT 会先读取资源元数据，再到该外部授权服务器执行授权码 + PKCE（S256）；外部服务必须支持 CIMD、DCR 或预注册客户端，并把下面的 MCP 地址写入访问令牌的 <code>aud</code>（或 <code>resource</code> claim）。
      </Notice>
      <CopyField label="ChatGPT 发现地址" value={metadataUrl} />
      <Notice tone="warning">只接受 RS256 的 RSA 公钥 JWKS。不要粘贴私钥、客户端密钥、访问令牌或对称密钥。</Notice>
      <form className="compact-form" onSubmit={async (event) => {
        event.preventDefault();
        setBusy(true);
        try {
          await adminApi.request('/mcp/oauth', { method: 'PUT', body: { ...issuerValues, discovery_url: null, enabled: true } });
          await refreshOAuth();
          notify('OAuth 发行方已保存。');
        } catch (error: unknown) {
          notify(formatRequestError(error), 'danger');
        } finally { setBusy(false); }
      }}>
        <h3>发行方与公钥</h3>
        <div className="form-grid">
          <label>名称<input required value={issuerValues.name} onChange={(event) => setIssuerValues({ ...issuerValues, name: event.target.value })} /></label>
          <label>Issuer URL<input required type="url" value={issuerValues.issuer_url} onChange={(event) => setIssuerValues({ ...issuerValues, issuer_url: event.target.value })} /></label>
          <label>Audience<input required value={issuerValues.audience} onChange={(event) => setIssuerValues({ ...issuerValues, audience: event.target.value })} /></label>
          <label>资源 URL<input readOnly required type="url" value={issuerValues.resource} /></label>
          <label className="form-span">RSA 公钥 JWKS<textarea required rows={6} value={issuerValues.jwks_cache_json} onChange={(event) => setIssuerValues({ ...issuerValues, jwks_cache_json: event.target.value })} placeholder='{"keys":[{"kty":"RSA","kid":"…","alg":"RS256","n":"…","e":"AQAB"}]}' /></label>
        </div>
        <p className="muted compact-text">通常将授权服务器的 API Audience 也设置为资源 URL；只有外部 IdP 明确使用另一 Audience 时才修改。</p>
        <button className="secondary-button" disabled={busy} type="submit">{busy ? '正在保存…' : '保存发行方'}</button>
      </form>

      <form className="compact-form" onSubmit={async (event) => {
        event.preventDefault();
        setBusy(true);
        try {
          await adminApi.request('/mcp/oauth/grants', { method: 'POST', body: { issuer_id: grantValues.issuer_id, subject: grantValues.subject, scopes: grantValues.scopes.split(',').map((value) => value.trim()).filter(Boolean) } });
          await refreshOAuth();
          notify('OAuth Subject 授权已保存。');
          setGrantValues((current) => ({ ...current, subject: '' }));
        } catch (error: unknown) {
          notify(formatRequestError(error), 'danger');
        } finally { setBusy(false); }
      }}>
        <h3>Subject 授权</h3>
        <div className="form-grid">
          <label>发行方<select required value={grantValues.issuer_id} onChange={(event) => setGrantValues({ ...grantValues, issuer_id: event.target.value })}><option value="">请选择</option>{issuers.map((issuer) => <option key={stringValue(issuer.id)} value={stringValue(issuer.id)}>{stringValue(issuer.name, stringValue(issuer.issuer_url))}</option>)}</select></label>
          <label>Subject<input required value={grantValues.subject} onChange={(event) => setGrantValues({ ...grantValues, subject: event.target.value })} /></label>
          <label className="form-span">Scopes（英文逗号分隔）<input required value={grantValues.scopes} onChange={(event) => setGrantValues({ ...grantValues, scopes: event.target.value })} /></label>
        </div>
        <button className="secondary-button" disabled={busy || issuers.length === 0} type="submit">{busy ? '正在保存…' : '保存 Subject 授权'}</button>
      </form>

      {grants.length > 0 ? (
        <div className="record-list">
          {grants.map((grant) => (
            <article className="record-item" key={stringValue(grant.id)}>
              <div className="record-main"><strong>{stringValue(grant.subject, '未知 Subject')}</strong><small>{Array.isArray(grant.scopes) ? grant.scopes.join(' · ') : '—'}</small></div>
              <button className="danger-link" type="button" disabled={busy} onClick={async () => {
                setBusy(true);
                try {
                  await adminApi.request(`/mcp/oauth/grants/${stringValue(grant.id, '')}`, { method: 'DELETE' });
                  await refreshOAuth();
                  notify('OAuth Subject 授权已撤销。');
                } catch (error: unknown) {
                  notify(formatRequestError(error), 'danger');
                } finally { setBusy(false); }
              }}>撤销</button>
            </article>
          ))}
        </div>
      ) : null}
    </div>
  );
}

type ProviderDeletionResult = {
  deleted: boolean;
  provider_id: string;
  models_deleted: number;
  bindings_deleted: number;
  embeddings_deleted: number;
  secrets_deleted: number;
};

function ProviderPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const providers = arrayRecords(data?.providers);
  const models = arrayRecords(data?.models);
  const bindings = arrayRecords(data?.bindings);
  const [deletingProviderId, setDeletingProviderId] = useState<string | null>(null);

  async function testProvider(provider: JsonObject) {
    try {
      const result = await adminApi.request<{ models: unknown[] }>(`/providers/${stringValue(provider.id, '')}/models/refresh`, { method: 'POST' });
      notify(`连接测试完成，发现 ${Array.isArray(result.models) ? result.models.length : 0} 个模型。`);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    }
  }

  async function deleteProvider(provider: JsonObject, providerModels: JsonObject[]) {
    const providerId = stringValue(provider.id, '');
    const providerName = stringValue(provider.name, '未命名服务');
    if (!providerId) {
      notify('AI 服务 ID 无效，请刷新页面后重试。', 'danger');
      return;
    }
    const modelIds = new Set(providerModels.map((model) => stringValue(model.id, '')).filter(Boolean));
    const visibleBindings = bindings.filter((binding) => modelIds.has(stringValue(binding.model_id, ''))).length;
    const confirmed = window.confirm(
      `确定删除“${providerName}”吗？\n\n` +
      `将删除 ${providerModels.length} 个模型，并解除所有 Vault 中使用这些模型的用途绑定` +
      `${visibleBindings > 0 ? `（当前可见 ${visibleBindings} 个）` : ''}；相关向量索引和该服务的加密密钥也会清理。\n\n` +
      'Vault 原始笔记、长期记忆、后台任务历史和审计记录不会被删除。此操作无法撤销。',
    );
    if (!confirmed) return;

    setDeletingProviderId(providerId);
    try {
      const revision = typeof provider.revision === 'number' && Number.isInteger(provider.revision)
        ? `?expected_revision=${provider.revision}`
        : '';
      const result = await adminApi.request<ProviderDeletionResult>(
        `/providers/${encodeURIComponent(providerId)}${revision}`,
        { method: 'DELETE' },
      );
      notify(
        `已删除“${providerName}”：清理 ${result.models_deleted} 个模型、` +
        `${result.bindings_deleted} 个用途绑定和 ${result.embeddings_deleted} 条可重建向量记录。`,
      );
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setDeletingProviderId(null);
    }
  }

  return (
    <div className="page-stack">
      <Panel title="数据发送策略" eyebrow="隐私优先" description="默认禁用。只有明确允许后，内容才可能发送给已配置的 AI 服务。">
        <ProviderModeForm data={asRecord(data?.provider_mode)} notify={notify} onRefresh={onRefresh} />
      </Panel>
      <Panel title="已配置的 AI 服务" eyebrow="提供商" description="API 密钥只保存为加密数据，页面不会返回原文。">
        {providers.length === 0 ? (
          <EmptyState title="尚未配置 AI 服务" detail="这不会影响 WebDAV、Vault 写入和全文搜索。" />
        ) : (
          <div className="record-list">
            {providers.map((provider) => {
              const health = asRecord(provider.health);
              const secret = asRecord(provider.secret);
              const providerModels = models.filter((model) => model.provider_id === provider.id);
              return (
                <article className="record-item record-item--stack" key={stringValue(provider.id)}>
                  <div className="record-title"><strong>{stringValue(provider.name)}</strong><StatusBadge tone={statusTone(health.status)}>{statusLabel(health.status)}</StatusBadge></div>
                  <p>{providerTypeLabel(provider.provider_type)} · <code>{stringValue(provider.base_url)}</code></p>
                  <small>{booleanValue(secret.configured) ? `密钥：${stringValue(secret.hint, '已配置')}` : '未配置密钥'} · 最近检查：{formatTime(health.checked_at)} · 模型 {providerModels.length} 个</small>
                  <div className="button-row">
                    <button className="secondary-button" disabled={deletingProviderId !== null} type="button" onClick={() => void testProvider(provider)}>发现/刷新模型</button>
                    <button
                      aria-label={`删除 AI 服务 ${stringValue(provider.name, '未命名服务')}`}
                      className="danger-button"
                      disabled={deletingProviderId !== null}
                      type="button"
                      onClick={() => void deleteProvider(provider, providerModels)}
                    >
                      {deletingProviderId === stringValue(provider.id, '') ? '正在删除…' : '删除 AI 服务'}
                    </button>
                  </div>
                  <details className="disclosure">
                    <summary>编辑 AI 服务</summary>
                    <EditProviderForm provider={provider} notify={notify} onRefresh={onRefresh} />
                  </details>
                  {providerModels.length > 0 ? (
                    <div className="record-list">
                      {providerModels.map((model) => (
                        <div className="summary-row" key={stringValue(model.id)}>
                          <span><code>{stringValue(model.external_model_id)}</code></span>
                          <strong>{modelCapabilityLabel(asRecord(model.capabilities))}{isOpenAiChatProvider(provider.provider_type) ? ` · ${openAiCompatibilityLabel(provider, model)}` : ''}</strong>
                        </div>
                      ))}
                    </div>
                  ) : <Notice tone="info">尚未登记模型。可以先尝试自动发现，也可以手动填写提供商要求的模型 ID。</Notice>}
                  <details className="disclosure">
                    <summary>手动登记模型</summary>
                    <ManualModelForm provider={provider} notify={notify} onRefresh={onRefresh} />
                  </details>
                </article>
              );
            })}
          </div>
        )}
        <details className="disclosure" open={providers.length === 0}>
          <summary>添加 AI 服务</summary>
          <AddProviderForm notify={notify} onRefresh={onRefresh} />
        </details>
      </Panel>
      <ModelBindingsPanel models={models} bindings={bindings} notify={notify} onRefresh={onRefresh} />
    </div>
  );
}

const modelRoles = [
  { value: 'memory_extraction', label: '自动生成长期记忆', detail: '从普通 Markdown 原文中识别并保存真正耐久的信息' },
  { value: 'memory_consolidation', label: '记忆整理', detail: '合并重复记忆并处理生命周期' },
  { value: 'note_summary', label: '笔记摘要', detail: '生成可重建的笔记摘要' },
  { value: 'topic_enrichment', label: '主题增强', detail: '辅助主题和知识结构分析' },
  { value: 'embedding_note', label: '笔记向量', detail: '让搜索和 recall 能按语义想起普通笔记' },
  { value: 'embedding_memory', label: '记忆向量', detail: '生成长期记忆语义向量' },
  { value: 'rerank', label: '结果重排', detail: '对候选检索结果进行可选重排' },
];

function ManualModelForm({ provider, notify, onRefresh }: { provider: JsonObject; notify: Notify; onRefresh: () => void }) {
  const [modelId, setModelId] = useState('');
  const [capability, setCapability] = useState('generation');
  const [dimension, setDimension] = useState('');
  const [contextWindow, setContextWindow] = useState('');
  const [maxOutputTokens, setMaxOutputTokens] = useState('');
  const [compatibilityPreset, setCompatibilityPreset] = useState('auto');
  const [structuredOutputMode, setStructuredOutputMode] = useState('auto');
  const [tokenLimitField, setTokenLimitField] = useState('auto');
  const [thinkingMode, setThinkingMode] = useState('auto');
  const [generationTokenLimit, setGenerationTokenLimit] = useState('');
  const [busy, setBusy] = useState(false);
  const effectivePreset = resolveCompatibilityPreset(
    compatibilityPreset,
    stringValue(provider.provider_type, ''),
    stringValue(provider.base_url, ''),
  );
  const supportsThinkingControl = providerPresetSupportsThinking(effectivePreset);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    const parsedDimension = dimension ? Number(dimension) : null;
    const parsedContextWindow = contextWindow ? Number(contextWindow) : null;
    const parsedMaxOutputTokens = maxOutputTokens ? Number(maxOutputTokens) : null;
    const parsedGenerationTokenLimit = generationTokenLimit ? Number(generationTokenLimit) : null;
    try {
      await adminApi.request(`/providers/${stringValue(provider.id, '')}/models`, {
        method: 'POST',
        body: {
          external_model_id: modelId,
          capabilities: {
            structured_output: capability === 'generation',
            embeddings: capability === 'embedding',
            reranking: capability === 'reranking',
            dimension: capability === 'embedding' ? parsedDimension : null,
            context_window: parsedContextWindow,
            max_output_tokens: capability === 'generation' ? parsedMaxOutputTokens : null,
          },
          settings: {
            openai_compatibility_preset: compatibilityPreset,
            openai_structured_output_mode: structuredOutputMode,
            openai_token_limit_field: tokenLimitField,
            openai_thinking_mode: supportsThinkingControl ? thinkingMode : 'auto',
            generation_token_limit: parsedGenerationTokenLimit,
          },
          enabled: true,
        },
      });
      notify('模型已登记，可以在下面绑定到具体用途。');
      setModelId('');
      setDimension('');
      setContextWindow('');
      setMaxOutputTokens('');
      setCompatibilityPreset('auto');
      setStructuredOutputMode('auto');
      setTokenLimitField('auto');
      setThinkingMode('auto');
      setGenerationTokenLimit('');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <form className="compact-form" onSubmit={submit}>
      <div className="form-grid">
        <label>模型 ID<input required value={modelId} onChange={(event) => setModelId(event.target.value)} placeholder="例如：gpt-5-mini 或 qwen3:8b" /></label>
        <label>主要能力<select value={capability} onChange={(event) => setCapability(event.target.value)}><option value="generation">结构化文本生成</option><option value="embedding">Embedding</option><option value="reranking">结果重排</option></select></label>
        {capability === 'embedding' ? <label>向量维度（可选）<input min="1" type="number" value={dimension} onChange={(event) => setDimension(event.target.value)} placeholder="例如：1536" /></label> : null}
        <label>上下文窗口（可选）<input min="1" type="number" value={contextWindow} onChange={(event) => setContextWindow(event.target.value)} placeholder="例如：128000" /></label>
      </div>
      {capability === 'generation' ? <details className="disclosure"><summary>高级：兼容模式与 Token 上限</summary><div className="form-grid">
        <label>模型声明的生成上限（可选）<input min="1" type="number" value={maxOutputTokens} onChange={(event) => setMaxOutputTokens(event.target.value)} placeholder="由模型平台文档提供" /></label>
        <label>单次生成 Token 上限（可选）<input min="1" max="1048576" type="number" value={generationTokenLimit} onChange={(event) => setGenerationTokenLimit(event.target.value)} placeholder="思考型预设默认 32768" /></label>
        {isOpenAiChatProvider(provider.provider_type) ? <label>提供商兼容预设<select value={compatibilityPreset} onChange={(event) => setCompatibilityPreset(event.target.value)}><option value="auto">跟随 AI 服务（推荐）</option><option value="generic">通用 OpenAI 兼容</option><option value="deepseek">DeepSeek</option><option value="xiaomi_mimo">小米 MiMo</option><option value="zhipu_glm">智谱 GLM</option><option value="moonshot_kimi">Kimi / Moonshot</option><option value="google_gemini">Google Gemini</option><option value="alibaba_qwen">阿里千问 / DashScope</option></select></label> : null}
        {isOpenAiChatProvider(provider.provider_type) ? <label>结构化输出方式<select value={structuredOutputMode} onChange={(event) => setStructuredOutputMode(event.target.value)}><option value="auto">跟随提供商预设</option><option value="strict_json_schema">严格 JSON Schema</option><option value="json_object">JSON Object</option><option value="prompt_only">仅提示词约束</option></select></label> : null}
        {isOpenAiChatProvider(provider.provider_type) ? <label>Token 上限字段<select value={tokenLimitField} onChange={(event) => setTokenLimitField(event.target.value)}><option value="auto">跟随提供商预设</option><option value="max_tokens">max_tokens</option><option value="max_completion_tokens">max_completion_tokens</option></select></label> : null}
        {isOpenAiChatProvider(provider.provider_type) && supportsThinkingControl ? <label>思考模式<select value={thinkingMode} onChange={(event) => setThinkingMode(event.target.value)}><option value="auto">模型/提供商默认</option><option value="enabled">开启</option><option value="disabled">关闭</option></select></label> : null}
      </div>{isOpenAiChatProvider(provider.provider_type) ? <small>默认按 AI 服务选择官方兼容契约；旧的通用配置只会从官方 API 域名迁移识别，不会根据本地模型名称猜接口。高级覆盖用于代理域名或平台文档明确不同的场景。</small> : null}</details> : null}
      <button className="secondary-button" disabled={busy} type="submit">{busy ? '正在登记…' : '登记模型'}</button>
    </form>
  );
}

function ModelBindingsPanel({ models, bindings, notify, onRefresh }: { models: JsonObject[]; bindings: JsonObject[]; notify: Notify; onRefresh: () => void }) {
  const primaryRole = modelRoles[0]!;
  const advancedRoles = modelRoles.slice(1);
  return (
    <Panel title="模型用途" eyebrow="角色绑定" description="同一个服务可以为不同任务选择不同模型；自动记忆必须先绑定模型。">
      {models.length === 0 ? (
        <EmptyState title="还没有可绑定模型" detail="先在上方发现或手动登记至少一个模型。" />
      ) : (
        <>
          <div className="record-list">
            <ModelBindingControl role={primaryRole} models={models} binding={bindings.find((item) => item.role === primaryRole.value)} notify={notify} onRefresh={onRefresh} />
          </div>
          <details className="disclosure">
            <summary>高级：摘要、Embedding 与重排模型</summary>
            <div className="record-list">
              {advancedRoles.map((role) => (
                <ModelBindingControl key={role.value} role={role} models={models} binding={bindings.find((item) => item.role === role.value)} notify={notify} onRefresh={onRefresh} />
              ))}
            </div>
          </details>
        </>
      )}
    </Panel>
  );
}

function ModelBindingControl({ role, models, binding, notify, onRefresh }: { role: { value: string; label: string; detail: string }; models: JsonObject[]; binding?: JsonObject; notify: Notify; onRefresh: () => void }) {
  const [selected, setSelected] = useState(stringValue(binding?.model_id, ''));
  const [busy, setBusy] = useState(false);

  useEffect(() => setSelected(stringValue(binding?.model_id, '')), [binding?.model_id]);

  async function save() {
    if (!selected) return;
    setBusy(true);
    try {
      const isVaultBinding = typeof binding?.vault_id === 'string' && binding.vault_id.length > 0;
      await adminApi.request(`/model-bindings/${role.value}`, {
        method: 'PUT',
        body: {
          model_id: selected,
          settings: {},
          expected_revision: isVaultBinding ? binding?.revision : null,
          vault_override: true,
        },
      });
      notify(`${role.label}模型已保存。`);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <article className="record-item">
      <div className="record-main">
        <div className="record-title"><strong>{role.label}</strong>{binding ? <StatusBadge tone="success">已绑定</StatusBadge> : <StatusBadge tone="warning">未绑定</StatusBadge>}</div>
        <small>{role.detail}</small>
        <select aria-label={`${role.label}模型`} value={selected} onChange={(event) => setSelected(event.target.value)}>
          <option value="">请选择模型</option>
          {models.map((model) => <option key={stringValue(model.id)} value={stringValue(model.id)}>{stringValue(model.external_model_id)} — {stringValue(model.provider_name, 'AI 服务')}</option>)}
        </select>
      </div>
      <button className="secondary-button" disabled={busy || !selected || selected === binding?.model_id} type="button" onClick={() => void save()}>{busy ? '正在保存…' : '保存'}</button>
    </article>
  );
}

function ProviderModeForm({ data, notify, onRefresh }: { data: JsonObject; notify: Notify; onRefresh: () => void }) {
  const [mode, setMode] = useState(stringValue(data.mode, 'disabled'));
  const [revision, setRevision] = useState<number | null>(typeof data.revision === 'number' ? data.revision : null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setMode(stringValue(data.mode, 'disabled'));
    setRevision(typeof data.revision === 'number' ? data.revision : null);
  }, [data.mode, data.revision]);

  async function save() {
    setBusy(true);
    try {
      const result = await adminApi.request<{ revision: number }>('/providers/mode', { method: 'PUT', body: { mode, expected_revision: revision ?? undefined } });
      setRevision(result.revision);
      notify('AI 数据发送策略已保存。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <div className="inline-form">
      <label>
        当前策略
        <select value={mode} onChange={(event) => setMode(event.target.value)}>
          <option value="disabled">禁用 AI 调用</option>
          <option value="local_only">仅允许本地地址</option>
          <option value="remote_allowed">允许远程 HTTPS</option>
        </select>
      </label>
      <button className="secondary-button" disabled={busy} type="button" onClick={() => void save()}>{busy ? '正在保存…' : '保存策略'}</button>
    </div>
  );
}

const providerBaseUrlDefaults: Record<string, string> = {
  openai_responses: 'https://api.openai.com/v1/',
  openai_compatible: '',
  deepseek: 'https://api.deepseek.com/v1/',
  xiaomi_mimo: 'https://api.xiaomimimo.com/v1/',
  zhipu_glm: 'https://open.bigmodel.cn/api/paas/v4/',
  moonshot_kimi: 'https://api.moonshot.ai/v1/',
  google_gemini: 'https://generativelanguage.googleapis.com/v1beta/openai/',
  alibaba_qwen: 'https://dashscope.aliyuncs.com/compatible-mode/v1/',
  anthropic_messages: 'https://api.anthropic.com/v1/',
  embedding_http: '',
};

const providerTypeOptions = [
  ['openai_responses', 'OpenAI'],
  ['anthropic_messages', 'Anthropic'],
  ['deepseek', 'DeepSeek'],
  ['xiaomi_mimo', '小米 MiMo'],
  ['zhipu_glm', '智谱 GLM'],
  ['moonshot_kimi', 'Kimi / Moonshot'],
  ['google_gemini', 'Google Gemini'],
  ['alibaba_qwen', '阿里千问 / DashScope'],
  ['openai_compatible', '其他 OpenAI 兼容服务'],
  ['embedding_http', '仅 Embedding HTTP'],
] as const;

function EditProviderForm({ provider, notify, onRefresh }: { provider: JsonObject; notify: Notify; onRefresh: () => void }) {
  const providerSettings = asRecord(provider.settings);
  const [values, setValues] = useState({
    name: stringValue(provider.name, ''),
    provider_type: stringValue(provider.provider_type, 'openai_compatible'),
    base_url: stringValue(provider.base_url, ''),
    enabled: booleanValue(provider.enabled),
    secret: '',
  });
  const [timeoutSeconds, setTimeoutSeconds] = useState(numberValue(providerSettings.timeout_ms, 30_000) / 1_000);
  const [connectTimeoutSeconds, setConnectTimeoutSeconds] = useState(numberValue(providerSettings.connect_timeout_ms, 5_000) / 1_000);
  const [maxRetries, setMaxRetries] = useState(numberValue(providerSettings.max_retries, 2));
  const [maxConcurrency, setMaxConcurrency] = useState(numberValue(providerSettings.max_concurrency, 4));
  const [allowPrivateNetworks, setAllowPrivateNetworks] = useState(booleanValue(providerSettings.allow_private_networks));
  const [organization, setOrganization] = useState(stringValue(providerSettings.organization, ''));
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const settings = asRecord(provider.settings);
    setValues({
      name: stringValue(provider.name, ''),
      provider_type: stringValue(provider.provider_type, 'openai_compatible'),
      base_url: stringValue(provider.base_url, ''),
      enabled: booleanValue(provider.enabled),
      secret: '',
    });
    setTimeoutSeconds(numberValue(settings.timeout_ms, 30_000) / 1_000);
    setConnectTimeoutSeconds(numberValue(settings.connect_timeout_ms, 5_000) / 1_000);
    setMaxRetries(numberValue(settings.max_retries, 2));
    setMaxConcurrency(numberValue(settings.max_concurrency, 4));
    setAllowPrivateNetworks(booleanValue(settings.allow_private_networks));
    setOrganization(stringValue(settings.organization, ''));
  }, [provider]);

  function selectProviderType(providerType: string) {
    const priorDefault = providerBaseUrlDefaults[values.provider_type] ?? '';
    const preserveCustomUrl = values.base_url.length > 0 && values.base_url !== priorDefault;
    setValues({
      ...values,
      provider_type: providerType,
      base_url: preserveCustomUrl ? values.base_url : (providerBaseUrlDefaults[providerType] ?? ''),
    });
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const providerId = stringValue(provider.id, '');
    if (!providerId) {
      notify('AI 服务 ID 无效，请刷新页面后重试。', 'danger');
      return;
    }
    setBusy(true);
    try {
      await adminApi.request(`/providers/${encodeURIComponent(providerId)}`, {
        method: 'PATCH',
        body: {
          name: values.name,
          provider_type: values.provider_type,
          base_url: values.base_url,
          enabled: values.enabled,
          secret: values.secret || null,
          expected_revision: typeof provider.revision === 'number' ? provider.revision : null,
          settings: {
            ...asRecord(provider.settings),
            timeout_ms: Math.round(timeoutSeconds * 1_000),
            connect_timeout_ms: Math.round(connectTimeoutSeconds * 1_000),
            max_retries: maxRetries,
            max_concurrency: maxConcurrency,
            allow_private_networks: allowPrivateNetworks,
            organization: organization || null,
          },
        },
      });
      notify(values.secret ? 'AI 服务配置和密钥已更新。' : 'AI 服务配置已更新，原密钥保持不变。');
      setValues((current) => ({ ...current, secret: '' }));
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <form aria-label={`编辑 ${stringValue(provider.name, '未命名服务')}`} className="compact-form" onSubmit={submit}>
      <div className="form-grid">
        <label>显示名称<input name="provider-name" required value={values.name} onChange={(event) => setValues({ ...values, name: event.target.value })} /></label>
        <label>AI 服务类型<select name="provider-type" value={values.provider_type} onChange={(event) => selectProviderType(event.target.value)}>{providerTypeOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <label>Base URL<input name="provider-base-url" required type="url" value={values.base_url} onChange={(event) => setValues({ ...values, base_url: event.target.value })} /></label>
        <label>替换 API 密钥（可选）<input autoComplete="new-password" name="provider-secret" type="password" value={values.secret} onChange={(event) => setValues({ ...values, secret: event.target.value })} placeholder="留空则保持当前密钥" /></label>
        <label className="checkbox-field"><input checked={values.enabled} name="provider-enabled" type="checkbox" onChange={(event) => setValues({ ...values, enabled: event.target.checked })} />启用此 AI 服务</label>
      </div>
      <details className="disclosure">
        <summary>高级：超时、重试与并发</summary>
        <div className="form-grid">
          <label>请求超时（秒）<input max="600" min="1" name="provider-timeout" required step="1" type="number" value={timeoutSeconds} onChange={(event) => setTimeoutSeconds(Number(event.target.value))} /></label>
          <label>连接超时（秒）<input max="600" min="1" name="provider-connect-timeout" required step="1" type="number" value={connectTimeoutSeconds} onChange={(event) => setConnectTimeoutSeconds(Number(event.target.value))} /></label>
          <label>瞬时错误重试次数<input max="8" min="0" name="provider-retries" required step="1" type="number" value={maxRetries} onChange={(event) => setMaxRetries(Number(event.target.value))} /></label>
          <label>最大并发请求<input max="64" min="1" name="provider-concurrency" required step="1" type="number" value={maxConcurrency} onChange={(event) => setMaxConcurrency(Number(event.target.value))} /></label>
          <label>组织/项目标识（可选）<input name="provider-organization" value={organization} onChange={(event) => setOrganization(event.target.value)} /></label>
          <label className="checkbox-field"><input checked={allowPrivateNetworks} name="provider-private-networks" type="checkbox" onChange={(event) => setAllowPrivateNetworks(event.target.checked)} />远程模式下允许显式配置的私有网络地址</label>
        </div>
      </details>
      <small>替换密钥留空时不会读取、覆盖或清除现有密钥。修改服务类型后请重新检查已登记模型是否仍兼容。</small>
      <button className="primary-button" disabled={busy} type="submit">{busy ? '正在保存…' : '保存修改'}</button>
    </form>
  );
}

function AddProviderForm({ notify, onRefresh }: { notify: Notify; onRefresh: () => void }) {
  const [values, setValues] = useState({ name: '', provider_type: 'openai_compatible', base_url: '', secret: '' });
  const [busy, setBusy] = useState(false);

  function selectProviderType(providerType: string) {
    const priorDefault = providerBaseUrlDefaults[values.provider_type] ?? '';
    const preserveCustomUrl = values.base_url.length > 0 && values.base_url !== priorDefault;
    setValues({
      ...values,
      provider_type: providerType,
      base_url: preserveCustomUrl ? values.base_url : (providerBaseUrlDefaults[providerType] ?? ''),
    });
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    try {
      await adminApi.request('/providers', { method: 'POST', body: { ...values, enabled: true, secret: values.secret || null, settings: {} } });
      notify('AI 服务已保存，密钥不会再次显示。');
      setValues({ name: '', provider_type: 'openai_compatible', base_url: '', secret: '' });
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <form className="compact-form" onSubmit={submit}>
      <div className="form-grid">
        <label>名称<input required value={values.name} onChange={(event) => setValues({ ...values, name: event.target.value })} placeholder="例如：本地 Ollama" /></label>
        <label>AI 服务类型<select value={values.provider_type} onChange={(event) => selectProviderType(event.target.value)}>{providerTypeOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <label>Base URL<input required type="url" value={values.base_url} onChange={(event) => setValues({ ...values, base_url: event.target.value })} placeholder="https://provider.example/v1/" /></label>
        <label>API 密钥（可选）<input type="password" value={values.secret} onChange={(event) => setValues({ ...values, secret: event.target.value })} autoComplete="new-password" /></label>
      </div>
      <button className="primary-button" disabled={busy} type="submit">{busy ? '正在保存…' : '安全保存 AI 服务'}</button>
    </form>
  );
}

function IndexPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const status = asRecord(data?.status);
  const noteSemantic = asRecord(data?.note_semantic);
  const semanticBlockers = Array.isArray(noteSemantic.blockers) ? noteSemantic.blockers.map(String) : [];
  const [busy, setBusy] = useState(false);

  async function rebuild() {
    setBusy(true);
    try {
      await adminApi.request('/index/rebuild', { method: 'POST' });
      notify('索引重建任务已加入后台队列。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <Panel title="全文与知识索引" eyebrow="可重建数据" description="索引损坏或删除不会影响 Markdown 原文件。" actions={<button className="secondary-button" type="button" disabled={busy} onClick={() => void rebuild()}>{busy ? '正在提交…' : '重建索引'}</button>}>
      {Object.keys(status).length === 0 ? (
        <EmptyState title="索引尚未建立" detail="首次扫描完成后会自动建立，也可以手动重建。" />
      ) : (
        <>
          <section className="metric-grid metric-grid--compact">
            <Metric label="覆盖率" value={formatPercent(status.coverage_ratio)} detail={`${numberValue(status.indexed_notes)} / ${numberValue(status.total_notes)} 篇 Markdown`} />
            <Metric label="Markdown" value={numberValue(status.indexed_notes)} detail="已索引笔记" />
            <Metric label="全部条目" value={numberValue(status.indexed_entries)} detail="包括附件元数据" />
            <Metric label="索引内容" value={formatBytes(status.indexed_bytes)} detail={`分析器 ${stringValue(status.analyzer_version)}`} />
            <Metric
              label="笔记语义召回"
              value={booleanValue(noteSemantic.configured) ? formatPercent(noteSemantic.coverage_ratio) : '未配置'}
              detail={booleanValue(noteSemantic.configured)
                ? `${numberValue(noteSemantic.indexed_chunks)} / ${numberValue(noteSemantic.source_chunks)} 个内容分块`
                : '绑定“笔记向量”模型后启用；当前仍可全文检索'}
            />
          </section>
          <p className="muted compact-text">上次完成：{formatTime(status.last_rebuilt_at)} · 索引版本：{numberValue(status.revision)}</p>
          {booleanValue(noteSemantic.configured) ? <p className="muted compact-text">语义模型：<code>{stringValue(noteSemantic.external_model_id, '模型记录缺失')}</code>{numberValue(noteSemantic.stale_vectors) > 0 ? ` · ${numberValue(noteSemantic.stale_vectors)} 条过期向量待重建` : ''}</p> : null}
          {semanticBlockers.length > 0 && booleanValue(noteSemantic.configured) ? <Notice tone="warning">语义召回尚未完全就绪：{semanticBlockers.map(noteSemanticBlockerLabel).join('；')}。</Notice> : null}
          {typeof status.last_error === 'string' && status.last_error ? <Notice tone="danger">最近错误：{status.last_error}</Notice> : null}
        </>
      )}
    </Panel>
  );
}

function noteSemanticBlockerLabel(code: string): string {
  const labels: Record<string, string> = {
    provider_service_unavailable: 'AI 服务边界不可用',
    provider_mode_disabled: 'AI 数据发送策略仍为禁用',
    model_binding_missing: '尚未绑定“笔记向量”模型',
    model_missing: '绑定的模型记录不存在',
    embedding_coverage_incomplete: '仍有笔记分块等待生成向量',
    semantic_status_unavailable: '暂时无法读取语义索引状态',
  };
  return labels[code] ?? code;
}

function MemoryPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const [memories, setMemories] = useState(() => arrayRecords(data?.memories));
  const extraction = asRecord(data?.extraction);
  const sourceHealth = asRecord(data?.source_health);
  const memoryJobs = arrayRecords(data?.memory_jobs);
  const [memoryActionId, setMemoryActionId] = useState('');

  useEffect(() => setMemories(arrayRecords(data?.memories)), [data?.memories]);

  async function changeMemory(memory: JsonObject, action: 'archive' | 'restore' | 'delete') {
    const id = stringValue(memory.id, '');
    const revision = numberValue(memory.revision);
    if (!id || revision <= 0) {
      notify('记忆 ID 或 revision 无效，请刷新后重试。', 'danger');
      return;
    }
    if (action === 'delete') {
      const content = stringValue(memory.content, '这条记忆').slice(0, 120);
      if (!window.confirm(`确定永久删除这条长期记忆吗？\n\n${content}\n\n当前规范 Markdown 和记忆投影会删除；修订历史或备份仍按保留策略存在。此操作不能通过“恢复”按钮撤销。`)) return;
    }
    setMemoryActionId(id);
    try {
      let updated: JsonObject | null = null;
      if (action === 'delete') {
        await adminApi.request(`/memories/${encodeURIComponent(id)}?expected_revision=${revision}`, { method: 'DELETE' });
      } else {
        updated = asRecord(await adminApi.request(`/memories/${encodeURIComponent(id)}/${action}`, {
          method: 'POST',
          body: { expected_revision: revision },
        }));
      }
      setMemories((current) => action === 'delete'
        ? current.filter((item) => stringValue(item.id) !== id)
        : current.map((item) => stringValue(item.id) === id
          ? (updated && Object.keys(updated).length > 0
              ? updated
              : { ...item, status: action === 'archive' ? 'archived' : 'active', revision: revision + 1 })
          : item));
      notify(action === 'archive' ? '长期记忆已归档，不再参与正常召回。' : action === 'restore' ? '长期记忆已恢复。' : '长期记忆已永久删除。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setMemoryActionId('');
    }
  }

  return (
    <div className="page-stack">
      <Notice tone="info">照常写笔记即可，不需要添加特殊标记或逐条审核。系统先从每篇笔记提炼带来源的原始记忆，再在后台合并、去重和处理冲突；只有整理后的语义内容会进入长期记忆。</Notice>
      <MemoryExtractionPanel data={extraction} jobs={memoryJobs} notify={notify} onRefresh={onRefresh} />
      <MemorySourceHealthPanel data={sourceHealth} notify={notify} onRefresh={onRefresh} />
      <Panel title={`长期记忆（${memories.length}）`} eyebrow="有来源的上下文" description="默认召回不调用在线模型。">
        {memories.length === 0 ? (
          <EmptyState title="还没有长期记忆" detail="Agent 主动记住或系统从普通笔记自动识别出的耐久信息会出现在这里。" />
        ) : (
          <div className="record-list">
            {memories.map((memory) => {
              const sources = arrayRecords(memory.sources);
              return (
                <article className="record-item record-item--stack" key={stringValue(memory.id)}>
                  <div className="record-title"><strong>{stringValue(memory.content, '无内容')}</strong><StatusBadge tone={statusTone(memory.status)}>{statusLabel(memory.status)}</StatusBadge></div>
                  <p>{memoryTypeLabel(memory.memory_type)} · 记忆文件 <code>{stringValue(memory.canonical_path)}</code></p>
                  {stringValue(memory.status_reason, '') ? <small>状态原因：{memoryStatusReasonLabel(memory.status_reason)}</small> : null}
                  <small>最近更新 {formatTime(memory.updated_at)}</small>
                  {sources.length > 0 ? (
                    <details className="disclosure memory-source-details">
                      <summary>查看来源笔记与证据定位（{sources.length}）</summary>
                      <div className="summary-list">
                        {sources.map((source, index) => (
                          <SummaryRow
                            key={`${stringValue(source.file_id, stringValue(source.source_type, 'source'))}-${index}`}
                            label={memorySourceTypeLabel(source.source_type)}
                            value={memorySourceLocation(source)}
                            mono={typeof source.path === 'string'}
                          />
                        ))}
                      </div>
                      <small>这里显示的是证据定位元数据；原文仍保留在对应笔记及其修订历史中，不会被复制成记忆正文。</small>
                    </details>
                  ) : <small>来源：已认证的显式记忆输入。</small>}
                  <div className="button-row">
                    {stringValue(memory.status) === 'active' ? <button aria-label={`归档长期记忆 ${stringValue(memory.id)}`} className="secondary-button" disabled={memoryActionId === stringValue(memory.id)} type="button" onClick={() => void changeMemory(memory, 'archive')}>归档</button> : null}
                    {['archived', 'stale', 'rejected'].includes(stringValue(memory.status)) ? <button aria-label={`恢复长期记忆 ${stringValue(memory.id)}`} className="secondary-button" disabled={memoryActionId === stringValue(memory.id)} type="button" onClick={() => void changeMemory(memory, 'restore')}>恢复</button> : null}
                    <button aria-label={`永久删除长期记忆 ${stringValue(memory.id)}`} className="danger-button" disabled={memoryActionId === stringValue(memory.id)} type="button" onClick={() => void changeMemory(memory, 'delete')}>永久删除</button>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </Panel>
    </div>
  );
}

function memorySourceTypeLabel(value: unknown): string {
  const labels: Record<string, string> = {
    note: 'Vault 笔记',
    explicit_agent: 'Agent 显式记忆',
    explicit_admin: 'Admin 显式记忆',
    direct_markdown: '托管 Markdown',
    import: '导入内容',
  };
  const sourceType = stringValue(value, 'unknown');
  return labels[sourceType] ?? sourceType;
}

function memorySourceLocation(source: JsonObject): string {
  const parts: string[] = [];
  const path = stringValue(source.path, '');
  if (path) parts.push(path);
  const revision = numberValue(source.revision);
  if (revision > 0) parts.push(`修订 ${revision}`);
  const startLine = numberValue(source.start_line);
  const endLine = numberValue(source.end_line);
  if (startLine > 0) parts.push(endLine > startLine ? `第 ${startLine}–${endLine} 行` : `第 ${startLine} 行`);
  const heading = Array.isArray(source.heading) ? source.heading.map(String).filter(Boolean).join(' › ') : '';
  if (heading) parts.push(`标题 ${heading}`);
  const health = stringValue(source.health, '');
  if (health) parts.push(`健康：${memorySourceHealthLabel(health)}`);
  return parts.join(' · ') || '已认证的显式输入（无笔记行号）';
}

function memoryStatusReasonLabel(value: unknown): string {
  const reason = stringValue(value);
  const labels: Record<string, string> = {
    source_unavailable: '没有可验证的当前来源笔记，已退出正常召回',
    source_retired: '来源变化已由记忆整理归档',
    superseded_by_consolidation: '已被更新的长期记忆替代',
    manual_archive: '由管理员或 Agent 明确归档',
    superseded: '已合并到另一条长期记忆',
  };
  return labels[reason] ?? reason;
}

function memorySourceHealthLabel(value: unknown): string {
  const health = stringValue(value, 'unverified');
  const labels: Record<string, string> = {
    current: '当前有效',
    unverified: '尚未核验',
    content_changed: '内容已变化',
    deleted: '来源文件已删除',
    identity_missing: '无法确认当前文件身份',
    identity_ambiguous: '存在多个精确候选',
  };
  return labels[health] ?? health;
}

function MemorySourceHealthPanel({ data, notify, onRefresh }: { data: JsonObject; notify: Notify; onRefresh: () => void }) {
  const [submitting, setSubmitting] = useState(false);
  const summary = asRecord(data.summary);
  const finalSources = asRecord(summary.final_sources);
  const memories = asRecord(summary.memories);
  const stage1 = asRecord(summary.stage1);
  const audit = asRecord(data.audit);
  const sources = arrayRecords(data.sources);

  async function runAudit() {
    setSubmitting(true);
    try {
      const job = asRecord(await adminApi.request('/memory/source-health/audit', { method: 'POST' }));
      notify(`来源健康审计任务 ${truncateId(stringValue(job.id))} 已提交。`);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Panel
      title="来源健康"
      eyebrow="持续核验"
      description="文件创建、更新、移动、删除和恢复都会先核验记忆来源；无法证明的记忆会保留用于审计，但不会进入正常召回。"
      actions={<button className="secondary-button" disabled={submitting} type="button" onClick={() => void runAudit()}>{submitting ? '正在提交…' : '重新审计全部来源'}</button>}
    >
      <div className="summary-list">
        <SummaryRow label="最终记忆来源" value={`${numberValue(finalSources.total)} 条（有效 ${numberValue(finalSources.current)} · 重绑 ${numberValue(finalSources.rebound)} · 内容变化 ${numberValue(finalSources.changed)} · 删除 ${numberValue(finalSources.deleted)} · 缺失 ${numberValue(finalSources.missing)} · 歧义 ${numberValue(finalSources.ambiguous)} · 未核验 ${numberValue(finalSources.unverified)}）`} />
        <SummaryRow label="受影响记忆" value={`${numberValue(memories.affected)} 条（保持 active ${numberValue(memories.active)} · 来源失效 stale ${numberValue(memories.stale)}）`} />
        <SummaryRow label="阶段一来源" value={`${numberValue(stage1.total)} 条（当前 ${numberValue(stage1.current)} · 已撤回 ${numberValue(stage1.withdrawn)} · 孤立 ${numberValue(stage1.orphaned)}）`} />
        <SummaryRow label="不同文件身份" value={`${numberValue(summary.distinct_file_ids)} 个 FileId`} />
      </div>
      {numberValue(finalSources.unverified) > 0 ? <Notice tone="warning">首次审计尚未覆盖所有来源。未核验的笔记依赖型记忆会暂时退出正常召回。</Notice> : null}
      {Object.keys(audit).length > 0 ? <small>最近审计：{stringValue(audit.status, '未知')} · 更新于 {formatTime(audit.updated_at)} · 代次 <code>{truncateId(stringValue(audit.generation))}</code></small> : <small>尚未记录完整来源审计。</small>}
      {sources.length > 0 ? (
        <details className="disclosure memory-source-details">
          <summary>查看来源健康明细样例（{sources.length}）</summary>
          <div className="record-list">
            {sources.map((source) => (
              <article className="record-item record-item--stack" key={stringValue(source.source_id)}>
                <div className="record-title"><strong>{memorySourceHealthLabel(source.health)}</strong><StatusBadge tone={stringValue(source.health) === 'current' ? 'success' : stringValue(source.health) === 'unverified' ? 'neutral' : 'warning'}>{memorySourceHealthLabel(source.health)}</StatusBadge></div>
                <p>来源笔记 <code>{stringValue(source.current_path, stringValue(source.recorded_path, '当前路径不可用'))}</code></p>
                <small>记忆 {truncateId(stringValue(source.memory_id))} · 来源 {truncateId(stringValue(source.source_id))} · 证据修订 {numberValue(source.evidence_revision)}</small>
                {stringValue(source.health_reason, '') ? <small>原因：{stringValue(source.health_reason)}</small> : null}
              </article>
            ))}
          </div>
        </details>
      ) : null}
    </Panel>
  );
}

function MemoryExtractionPanel({ data, jobs, notify, onRefresh }: { data: JsonObject; jobs: JsonObject[]; notify: Notify; onRefresh: () => void }) {
  const policy = asRecord(data.policy);
  const readiness = asRecord(data.readiness);
  const phase1Readiness = asRecord(data.phase1_readiness);
  const phase2Readiness = asRecord(data.phase2_readiness);
  const stage1 = asRecord(data.stage1);
  const consolidation = asRecord(data.consolidation);
  const blockers = Array.isArray(readiness.blockers) ? readiness.blockers.map(String) : [];
  const [enabled, setEnabled] = useState(booleanValue(policy.enabled));
  const [requestTimeoutSeconds, setRequestTimeoutSeconds] = useState(numberValue(policy.request_timeout_seconds, 300));
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [submittedJob, setSubmittedJob] = useState<JsonObject | null>(null);
  const [operationMessage, setOperationMessage] = useState<{ text: string; tone: NoticeTone } | null>(null);

  const submittedJobId = stringValue(submittedJob?.id, '');
  const visibleJobs = submittedJob && !jobs.some((job) => stringValue(job.id) === submittedJobId)
    ? [submittedJob, ...jobs]
    : jobs;
  const activeJob = visibleJobs.find((job) => ['queued', 'running', 'retry_wait'].includes(stringValue(job.status)));

  useEffect(() => {
    if (dirty) return;
    setEnabled(booleanValue(policy.enabled));
    setRequestTimeoutSeconds(numberValue(policy.request_timeout_seconds, 300));
  }, [dirty, policy.enabled, policy.request_timeout_seconds]);

  useEffect(() => {
    if (submittedJobId && jobs.some((job) => stringValue(job.id) === submittedJobId)) setSubmittedJob(null);
  }, [jobs, submittedJobId]);

  async function save() {
    setBusy(true);
    try {
      await adminApi.request('/memory/extraction', {
        method: 'PUT',
        body: {
          enabled,
          source_mode: 'automatic',
          request_timeout_seconds: requestTimeoutSeconds,
          expected_revision: typeof data.revision === 'number' ? data.revision : null,
        },
      });
      setOperationMessage({ text: '两阶段记忆设置已保存。', tone: 'success' });
      notify('两阶段记忆设置已保存。');
      setDirty(false);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  async function processExistingNotes(includeEvaluated: boolean) {
    const prompt = includeEvaluated
      ? '确定重新提取全部现有笔记吗？未修改且已经成功提取的笔记也会再次调用提取模型，然后由整理模型重新合并；这会增加 Token 消耗。'
      : '确定处理新增、内容有变化、配置有变化或上次失败的笔记吗？未变化且已经成功提取的笔记不会再次调用模型。';
    if (!window.confirm(prompt)) return;
    setBusy(true);
    try {
      const job = asRecord(await adminApi.request('/memory/extraction/run', {
        method: 'POST',
        body: { include_evaluated: includeEvaluated },
      }));
      const jobId = stringValue(job.id, '未知任务');
      const reused = stringValue(job.admission) === 'existing';
      setSubmittedJob(job);
      const text = reused
        ? `已有记忆任务 ${truncateId(jobId)} 正在执行，没有重复创建。`
        : includeEvaluated
          ? `任务 ${truncateId(jobId)} 已开始重新提取全部笔记，提取完成后会自动整理。`
          : `任务 ${truncateId(jobId)} 已开始处理新增或变化的笔记，提取完成后会自动整理。`;
      setOperationMessage({ text, tone: 'success' });
      notify(text);
      onRefresh();
    } catch (error: unknown) {
      const text = formatRequestError(error);
      setOperationMessage({ text, tone: 'danger' });
      notify(text, 'danger');
    } finally { setBusy(false); }
  }

  const pipelineReady = booleanValue(readiness.ready);
  const configurationReady = booleanValue(phase1Readiness.ready) && booleanValue(phase2Readiness.ready);
  const regenerationPending = booleanValue(consolidation.regeneration_pending);
  return (
    <Panel
      title="两阶段长期记忆"
      eyebrow="先提取 · 再整理"
      description="阶段一逐篇提炼原始记忆，并由本地绑定笔记来源修订；阶段二在 Vault 范围内合并、去重、处理冲突，并写入最终语义记忆。"
      actions={<StatusBadge tone={pipelineReady ? 'success' : 'warning'}>{pipelineReady ? '可以运行' : configurationReady && regenerationPending ? '准备重新生成' : '尚未就绪'}</StatusBadge>}
    >
      <div className="compact-form">
        <div className="choice-group">
          <Choice checked={enabled} label="自动处理笔记变更" detail="笔记新建或更新后自动执行阶段一，并在后台触发阶段二整理" onChange={() => { setEnabled((value) => !value); setDirty(true); }} />
        </div>
        <div className="button-row">
          <button className="secondary-button" disabled={busy || !dirty} type="button" onClick={() => void save()}>{busy ? '正在保存…' : '保存设置'}</button>
        </div>
      </div>
      <Notice tone="info">提取模型不负责返回证据行号；服务会把原始记忆绑定到当前笔记及其修订。最终记忆是模型归纳后的简短语义，阶段二会自动决定保留、合并、更新或遗忘，不存在“待审核候选”，也不需要人工逐条确认。</Notice>
      {operationMessage ? <Notice tone={operationMessage.tone}>{operationMessage.text}</Notice> : null}
      <div className="summary-list">
        <SummaryRow label="阶段一 · 提取模型" value={stringValue(phase1Readiness.external_model_id, '未绑定')} mono />
        <SummaryRow label="阶段二 · 整理模型" value={stringValue(phase2Readiness.external_model_id, '未绑定')} mono />
        <SummaryRow label="已处理笔记来源" value={`${numberValue(stage1.total)}（有原始记忆 ${numberValue(stage1.ready)} · 无需记忆 ${numberValue(stage1.no_output)}）`} />
        <SummaryRow label="等待整理的原始输入" value={numberValue(stage1.pending)} />
        <SummaryRow label="已提交全局记忆版本" value={numberValue(consolidation.generation)} />
        <SummaryRow label="最近整理完成" value={formatTime(consolidation.last_success_at)} />
      </div>
      {numberValue(consolidation.pipeline_generation) < 1 ? <Notice tone="warning">旧版记忆系统正在整体作废并清理；普通 Vault 笔记不会删除，清理完成后会从第 1 篇重新提取。</Notice> : null}
      {numberValue(consolidation.pipeline_generation) >= 1 && regenerationPending ? <Notice tone="warning">旧版记忆和任务已清理；配置就绪后会立即创建全量任务，也可以点击下方按钮立即触发。</Notice> : null}
      {blockers.length > 0 ? <Notice tone="warning">{blockers.map(extractionBlockerLabel).join('；')}。</Notice> : null}
      <details className="disclosure">
        <summary>高级设置</summary>
        <div className="form-grid">
          <label>单篇提取超时（秒，30–1800）<input max="1800" min="30" step="1" type="number" value={requestTimeoutSeconds} onChange={(event) => { setRequestTimeoutSeconds(Number(event.target.value)); setDirty(true); }} /></label>
        </div>
      </details>
      <div className="button-row">
        <button className="primary-button" disabled={busy || Boolean(activeJob) || !enabled || !configurationReady} type="button" onClick={() => void processExistingNotes(false)}>{activeJob ? `任务执行中 · ${jobProgressLabel(activeJob)}` : regenerationPending ? '立即开始全量生成' : '处理新增或变化的笔记'}</button>
        <button className="secondary-button" disabled={busy || Boolean(activeJob) || !enabled || !configurationReady} type="button" onClick={() => void processExistingNotes(true)}>重新提取全部笔记</button>
      </div>
      {visibleJobs.length > 0 ? (
        <details className="disclosure" open={Boolean(activeJob)}>
          <summary>最近的记忆任务（{visibleJobs.length}）</summary>
          <div className="record-list">
            {visibleJobs.slice(0, 12).map((job) => (
              <div className="summary-row" key={stringValue(job.id)}>
                <span>
                  <strong>{jobTypeLabel(job.job_type)}</strong> · <code>{truncateId(job.id)}</code> · {formatTime(job.updated_at)}
                  <small>{jobProgressDetail(job)}</small>
                  {typeof job.last_error === 'string' && job.last_error ? <small>{jobErrorContextLabel(job)}</small> : null}
                </span>
                <strong>{jobStatusLabel(job)} · {jobProgressLabel(job)}</strong>
              </div>
            ))}
          </div>
        </details>
      ) : null}
    </Panel>
  );
}

function extractionBlockerLabel(code: string): string {
  const labels: Record<string, string> = {
    memory_pipeline_reset_pending: '新版记忆系统正在清理旧数据并准备从头生成',
    memory_pipeline_regeneration_pending: '新版记忆系统正在创建必须的全量重新提取任务',
    extraction_disabled: '尚未启用自动记忆',
    provider_mode_disabled: 'AI 数据发送策略仍为禁用',
    model_binding_missing: '尚未绑定阶段一“记忆提取”模型',
    model_missing: '阶段一绑定的模型记录不存在',
    model_disabled: '阶段一绑定的模型已停用',
    provider_missing: '阶段一模型所属 AI 服务不存在',
    provider_disabled: '阶段一模型所属 AI 服务已停用',
    consolidation_model_binding_missing: '尚未绑定阶段二“记忆整理”模型',
    consolidation_model_missing: '阶段二绑定的模型记录不存在',
    consolidation_model_disabled: '阶段二绑定的模型已停用',
    consolidation_provider_missing: '阶段二模型所属 AI 服务不存在',
    consolidation_provider_disabled: '阶段二模型所属 AI 服务已停用',
  };
  return labels[code] ?? code;
}

function JobsPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const running = arrayRecords(data?.running);
  const queued = arrayRecords(data?.queued);
  const retryWait = arrayRecords(data?.retry_wait);
  const history = arrayRecords(data?.history);
  const counts = asRecord(data?.counts);
  const truncated = asRecord(data?.truncated);
  const activeCount = numberValue(counts.active, running.length + queued.length + retryWait.length);
  const terminalCount = numberValue(counts.terminal, history.length);

  async function act(job: JsonObject, action: 'retry' | 'cancel') {
    if (action === 'cancel' && !window.confirm('确定请求取消这个后台任务吗？正在执行的安全关键任务可能拒绝取消。')) return;
    try {
      await adminApi.request(`/jobs/${stringValue(job.id, '')}/${action}`, { method: 'POST' });
      notify(action === 'retry' ? '任务已重新加入队列。' : '取消请求已提交。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    }
  }

  function records(jobs: JsonObject[]) {
    return (
      <div className="record-list">
        {jobs.map((job) => {
          const status = stringValue(job.status, 'unknown');
          const jobType = stringValue(job.job_type);
          const protectedWhileRunning = status === 'running' && ['memory.consolidate', 'memory.reset_pipeline'].includes(jobType);
          const cancellable = ['queued', 'running', 'retry_wait'].includes(status) && !protectedWhileRunning;
          return (
            <article className="record-item" key={stringValue(job.id)}>
              <div className="record-main">
                <div className="record-title"><strong>{jobTypeLabel(job.job_type)}</strong><StatusBadge tone={jobStatusTone(job)}>{jobStatusLabel(job)}</StatusBadge></div>
                <p><code>{truncateId(job.id)}</code> · {jobProgressDetail(job)}</p>
                <small>尝试 {numberValue(job.attempts)} / {numberValue(job.max_attempts)} · 更新于 {formatTime(job.updated_at)}{typeof job.last_error === 'string' && job.last_error ? ` · ${jobErrorContextLabel(job)}` : ''}</small>
              </div>
              <div className="button-column">
                {status === 'failed' ? <button className="secondary-button" type="button" onClick={() => void act(job, 'retry')}>重试</button> : null}
                {cancellable ? <button className="danger-link" type="button" onClick={() => void act(job, 'cancel')}>取消</button> : null}
              </div>
            </article>
          );
        })}
      </div>
    );
  }

  return (
    <Panel title="后台任务" eyebrow="持久化队列" description="正在执行的任务始终置顶显示；等待任务和已结束历史不会再把它们挤出页面。">
      {activeCount === 0 && terminalCount === 0 ? (
        <EmptyState title="当前没有任务" detail="扫描、索引、备份和记忆处理任务会显示在这里。" />
      ) : (
        <div className="job-groups">
          <section className="job-group" aria-labelledby="running-jobs-title">
            <div className="job-group-heading">
              <div><p className="eyebrow">当前占用 Worker</p><h3 id="running-jobs-title">正在执行（{numberValue(counts.running, running.length)}）</h3></div>
              <StatusBadge tone={running.length > 0 ? 'warning' : 'neutral'}>{running.length > 0 ? '自动刷新' : '当前空闲'}</StatusBadge>
            </div>
            {running.length > 0 ? records(running) : <p className="muted-copy">当前没有正在执行的任务。</p>}
            {booleanValue(truncated.running) ? <Notice tone="warning">运行任务超过页面安全上限，请通过单个任务接口继续诊断。</Notice> : null}
          </section>

          <section className="job-group" aria-labelledby="waiting-jobs-title">
            <div className="job-group-heading">
              <div><p className="eyebrow">尚未占用 Worker</p><h3 id="waiting-jobs-title">等待与重试（{numberValue(counts.queued, queued.length) + numberValue(counts.retry_wait, retryWait.length)}）</h3></div>
            </div>
            {queued.length > 0 ? <><h4>等待执行（{numberValue(counts.queued, queued.length)}）</h4>{records(queued)}</> : null}
            {retryWait.length > 0 ? <><h4>等待重试（{numberValue(counts.retry_wait, retryWait.length)}）</h4>{records(retryWait)}</> : null}
            {queued.length === 0 && retryWait.length === 0 ? <p className="muted-copy">当前没有排队或等待重试的任务。</p> : null}
            {booleanValue(truncated.queued) || booleanValue(truncated.retry_wait) ? <Notice tone="info">这里显示前 {queued.length + retryWait.length} 条，队列总数以上方数字为准。</Notice> : null}
          </section>

          <section className="job-group" aria-labelledby="history-jobs-title">
            <div className="job-group-heading">
              <div><p className="eyebrow">不会影响当前调度</p><h3 id="history-jobs-title">已结束历史（显示 {history.length} / 共 {terminalCount}）</h3></div>
            </div>
            {history.length > 0 ? records(history) : <p className="muted-copy">还没有已完成、失败或取消的任务。</p>}
          </section>
        </div>
      )}
    </Panel>
  );
}

function jobProgressLabel(job: JsonObject): string {
  const status = stringValue(job.status, 'unknown');
  if (status === 'completed') return '100%';
  const projected = progressRatio(job.progress_ratio) ?? progressRatio(job.progress);
  return projected === null ? '未报告' : formatPercent(projected);
}

function jobStatusLabel(job: JsonObject): string {
  const status = stringValue(job.status, 'unknown');
  if (booleanValue(job.cancel_requested) && ['queued', 'running', 'retry_wait'].includes(status)) return '正在取消';
  if (status === 'completed' && jobHasNoteFailures(job)) return '完成但有失败';
  return statusLabel(status);
}

function jobStatusTone(job: JsonObject): 'success' | 'warning' | 'danger' | 'neutral' {
  const status = stringValue(job.status, 'unknown');
  if (status === 'completed' && jobHasNoteFailures(job)) return 'warning';
  return statusTone(status);
}

function jobHasNoteFailures(job: JsonObject): boolean {
  const progress = asRecord(job.progress);
  return stringValue(progress.phase, '') === 'completed_with_errors'
    || numberValue(progress.source_ingestion_failures) > 0
    || numberValue(progress.generated_output_failures) > 0;
}

function jobProgressDetail(job: JsonObject): string {
  const progress = asRecord(job.progress);
  const details = asRecord(job.details);
  const phase = stringValue(progress.phase, '');
  const completed = numberValue(progress.completed);
  const total = numberValue(progress.total);
  const currentIndex = numberValue(progress.current_index);
  const currentPath = stringValue(progress.current_path, '');
  const rawMemoriesStaged = numberValue(progress.raw_memories_staged);
  const phase1NoOutput = numberValue(progress.phase1_no_output);
  const sourceIngestionFailures = numberValue(progress.source_ingestion_failures);
  const sourceIngestionFailureNotes = arrayRecords(progress.source_ingestion_failure_notes);
  const generatedOutputFailures = numberValue(progress.generated_output_failures);
  const generatedOutputFailureNotes = arrayRecords(progress.generated_output_failure_notes);
  const notesEvaluated = numberValue(progress.notes_evaluated);
  const sourcePolicySkipped = numberValue(progress.source_policy_skipped);
  const alreadyEvaluatedSkipped = numberValue(progress.already_evaluated_skipped);
  const created = numberValue(progress.created);
  const updated = numberValue(progress.updated);
  const retired = numberValue(progress.retired);
  const discarded = numberValue(progress.discarded);
  const pendingRawInputs = numberValue(progress.pending_raw_inputs);
  const generation = numberValue(progress.generation);
  const memoriesRewritten = numberValue(progress.memories_rewritten);
  const stage1SourcesRebound = numberValue(progress.stage1_sources_rebound);
  const unresolvedNoteSources = numberValue(progress.unresolved_note_sources);
  const memoriesMarkedStale = numberValue(progress.memories_marked_stale);
  const noteStartedAt = numberValue(progress.note_started_at);
  const lastNoteElapsedMs = numberValue(progress.last_note_elapsed_ms);
  const sourceCounts = asRecord(progress.counts);
  const auditedSources = numberValue(sourceCounts.final_sources_checked);
  const sourceErrors = numberValue(sourceCounts.errors);

  let detail: string;
  if (phase === 'consolidating') {
    detail = `正在整理原始记忆：已处理 ${completed} / ${total || completed + pendingRawInputs} 条${pendingRawInputs > 0 ? `，仍待 ${pendingRawInputs} 条` : ''}`;
  } else if (phase === 'repairing_memory_sources') {
    detail = '正在核对历史记忆的文件身份和当前路径';
  } else if (phase === 'auditing_sources') {
    detail = `正在持续核验记忆来源：已处理 ${auditedSources} 条最终来源`;
  } else if (phase === 'resetting_memory_pipeline') {
    detail = '正在清空旧版记忆系统';
  } else if (phase === 'extracting_note') {
    detail = `正在处理第 ${currentIndex || completed + 1} / ${total || 1} 篇${currentPath ? `：${currentPath}` : ''}`;
  } else if (phase === 'waiting_retry' || phase === 'failed') {
    detail = `第 ${currentIndex || completed + 1} / ${total || 1} 篇未完成${currentPath ? `：${currentPath}` : ''}`;
  } else if (phase === 'stopped_output_failures') {
    detail = `已处理 ${completed} / ${total} 篇，因连续模型输出错误暂停`;
  } else if (phase === 'enumerated') {
    detail = `已发现 ${total} 篇 Markdown，准备开始`;
  } else if (phase === 'completed' && stringValue(job.job_type) === 'memory.consolidate') {
    detail = `已完成第 ${generation} 版全局记忆整理`;
  } else if (phase === 'completed' && stringValue(job.job_type) === 'memory.reset_pipeline') {
    detail = '旧版记忆和任务已作废，准备从头生成';
  } else if (phase === 'completed' && stringValue(job.job_type) === 'memory.repair_sources') {
    detail = `已重写 ${memoriesRewritten} 条记忆来源，更新 ${stage1SourcesRebound} 条阶段一来源`;
  } else if (['completed', 'completed_with_errors'].includes(phase) && stringValue(job.job_type) === 'memory.audit_sources') {
    detail = `来源审计完成：核验 ${auditedSources} 条最终来源${sourceErrors > 0 ? `，${sourceErrors} 条未能安全处理` : ''}`;
  } else if (phase === 'completed' && stringValue(job.job_type) === 'memory.source_reconcile') {
    detail = `文件事件来源协调完成：核验 ${numberValue(progress.final_sources_checked)} 条最终来源`;
  } else if (phase === 'note_completed' || phase === 'completed' || phase === 'completed_with_errors') {
    detail = `已处理 ${completed} / ${total} 篇`;
  } else if (stringValue(details.scope, '') === 'all') {
    detail = '等待扫描当前 Vault 的 Markdown';
  } else if (stringValue(details.source_path, '')) {
    detail = `等待处理：${stringValue(details.source_path, '')}`;
  } else {
    detail = `进度 ${jobProgressLabel(job)}`;
  }

  const outcomes = [];
  if (phase === 'extracting_note' && noteStartedAt > 0) {
    outcomes.push(`本篇已处理 ${formatJobDuration(Date.now() - noteStartedAt)}`);
  } else if (lastNoteElapsedMs > 0) {
    outcomes.push(`本篇耗时 ${formatJobDuration(lastNoteElapsedMs)}`);
  }
  if (notesEvaluated > 0) outcomes.push(`模型处理 ${notesEvaluated} 篇`);
  if (rawMemoriesStaged > 0) outcomes.push(`提炼原始记忆 ${rawMemoriesStaged} 篇`);
  if (phase1NoOutput > 0) outcomes.push(`无需形成记忆 ${phase1NoOutput} 篇`);
  if (sourcePolicySkipped > 0) outcomes.push(`处理前跳过 ${sourcePolicySkipped} 篇`);
  if (alreadyEvaluatedSkipped > 0) outcomes.push(`未变化且已处理，跳过模型 ${alreadyEvaluatedSkipped} 篇`);
  if (created > 0) outcomes.push(`新增长期记忆 ${created} 条`);
  if (updated > 0) outcomes.push(`更新长期记忆 ${updated} 条`);
  if (retired > 0) outcomes.push(`归档或替代 ${retired} 条`);
  if (discarded > 0) outcomes.push(`丢弃低价值原始输入 ${discarded} 条`);
  if (memoriesMarkedStale > 0) outcomes.push(`标记失效记忆 ${memoriesMarkedStale} 条`);
  if (unresolvedNoteSources > 0) outcomes.push(`仍有 ${unresolvedNoteSources} 条来源无法证明当前文件身份`);
  if (sourceIngestionFailures > 0) outcomes.push(`源文件无法处理 ${sourceIngestionFailures} 篇（模型未调用）`);
  const latestSourceFailure = sourceIngestionFailureNotes.length > 0
    ? sourceIngestionFailureNotes[sourceIngestionFailureNotes.length - 1]
    : null;
  if (latestSourceFailure) {
    const failurePath = stringValue(latestSourceFailure.path, '未知笔记');
    const errorCode = stringValue(latestSourceFailure.error_code, 'memory_source_read_failed');
    outcomes.push(`最近源文件问题 ${failurePath}：${jobErrorLabel(errorCode)}`);
  }
  if (generatedOutputFailures > 0) outcomes.push(`模型输出校验失败 ${generatedOutputFailures} 篇（模型已调用）`);
  const latestGeneratedFailure = generatedOutputFailureNotes.length > 0
    ? generatedOutputFailureNotes[generatedOutputFailureNotes.length - 1]
    : null;
  if (latestGeneratedFailure) {
    const failurePath = stringValue(latestGeneratedFailure.path, '未知笔记');
    const errorCode = stringValue(latestGeneratedFailure.error_code, 'provider_response_invalid');
    const schemaIssue = stringValue(latestGeneratedFailure.schema_issue, '');
    const schemaPath = stringValue(latestGeneratedFailure.schema_path, '');
    const schemaDetail = schemaIssue ? `（${schemaViolationLabel(schemaIssue, schemaPath)}）` : '';
    outcomes.push(`最近模型输出问题 ${failurePath}：${jobErrorLabel(errorCode)}${schemaDetail}`);
  }
  if (numberValue(progress.removed_managed_files) > 0) outcomes.push(`删除旧版托管记忆文件 ${numberValue(progress.removed_managed_files)} 个`);
  if (numberValue(progress.cleared_memories) > 0) outcomes.push(`清空旧版长期记忆 ${numberValue(progress.cleared_memories)} 条`);
  if (numberValue(progress.cleared_stage1_outputs) > 0) outcomes.push(`清空旧版原始记忆 ${numberValue(progress.cleared_stage1_outputs)} 条`);
  if (booleanValue(details.include_evaluated)) outcomes.push('任务模式：重新提取全部笔记');
  if (phase === 'completed' && total === 0 && stringValue(job.job_type) === 'memory.extract') outcomes.push('没有 Markdown 笔记');
  return outcomes.length > 0 ? `${detail} · ${outcomes.join(' · ')}` : detail;
}

function schemaViolationLabel(issue: string, path: string): string {
  const location = path || '返回对象';
  const labels: Record<string, string> = {
    type_mismatch: `${location} 的类型不正确`,
    enum_mismatch: `${location} 不在允许值中`,
    required_property_missing: `缺少必填字段 ${location}`,
    unexpected_property: `${location} 包含未允许的字段`,
    array_too_long: `${location} 的项目数超过上限`,
    array_too_short: `${location} 的项目数不足`,
    schema_invalid: `${location} 的校验规则无效`,
  };
  return labels[issue] ?? `${location} 未通过结构校验`;
}

function formatJobDuration(milliseconds: number): string {
  const seconds = Math.max(0, Math.round(milliseconds / 1_000));
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  const remaining = seconds % 60;
  return remaining === 0 ? `${minutes} 分钟` : `${minutes} 分 ${remaining} 秒`;
}

function jobErrorContextLabel(job: JsonObject): string {
  const status = stringValue(job.status, 'unknown');
  const prefix = status === 'running' || status === 'queued' ? '上次尝试' : '失败原因';
  return `${prefix}：${jobErrorLabel(job.last_error)}`;
}

function progressRatio(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return Math.max(0, Math.min(1, value > 1 ? value / 100 : value));
  }
  const progress = asRecord(value);
  const direct = typeof progress.ratio === 'number'
    ? progress.ratio
    : typeof progress.percent === 'number'
      ? progress.percent / 100
      : null;
  if (direct !== null && Number.isFinite(direct)) return Math.max(0, Math.min(1, direct));
  const completed = typeof progress.completed === 'number'
    ? progress.completed
    : typeof progress.done === 'number'
      ? progress.done
      : null;
  const total = typeof progress.total === 'number' ? progress.total : null;
  if (completed === null || total === null || total <= 0) return null;
  return Math.max(0, Math.min(1, completed / total));
}

function AuditPage({ data }: { data: JsonObject | null }) {
  const entries = arrayRecords(data?.entries);
  return (
    <Panel title={`最近操作（${entries.length}）`} eyebrow="脱敏审计" description="不记录密码、Token、API Key、笔记正文或记忆正文。">
      {entries.length === 0 ? (
        <EmptyState title="暂无审计记录" detail="管理端和协议操作产生的安全事件会显示在这里。" />
      ) : (
        <div className="timeline-list">
          {entries.map((entry) => (
            <article className="timeline-item" key={stringValue(entry.id)}>
              <span className={`timeline-dot timeline-dot--${statusTone(entry.result)}`} aria-hidden="true" />
              <div>
                <div className="record-title"><strong>{auditActionLabel(entry.action)}</strong><StatusBadge tone={stringValue(entry.result) === 'success' ? 'success' : 'danger'}>{stringValue(entry.result) === 'success' ? '成功' : statusLabel(entry.result)}</StatusBadge></div>
                <p>{stringValue(entry.actor_type, 'system')} · {stringValue(entry.plane)} · {formatTime(entry.occurred_at)}</p>
                <small>请求 ID：<code>{truncateId(entry.request_id)}</code></small>
              </div>
            </article>
          ))}
        </div>
      )}
    </Panel>
  );
}

function BackupPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const backups = arrayRecords(data?.backups);
  const firstBackupId = stringValue(backups[0]?.id, '');
  const [selected, setSelected] = useState(firstBackupId);
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!selected && firstBackupId) setSelected(firstBackupId);
  }, [firstBackupId, selected]);

  async function run(path: string, options: { method: string; body?: unknown }, message: string) {
    setBusy(true);
    try {
      await adminApi.request<JsonObject>(path, options);
      notify(message);
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(false); }
  }

  return (
    <Panel title="已验证备份" eyebrow="恢复保障" description="普通备份不包含安装主密钥，请单独保存主密钥。" actions={<button className="primary-button" type="button" disabled={busy} onClick={() => void run('/backups', { method: 'POST' }, '备份任务已加入后台队列。')}>{busy ? '正在提交…' : '创建新备份'}</button>}>
      {backups.length === 0 ? (
        <EmptyState title="还没有备份" detail="点击“创建新备份”，任务会在后台执行并验证产物。" />
      ) : (
        <div className="record-list">
          {backups.map((backup) => {
            const manifest = asRecord(backup.manifest);
            return (
              <article className="record-item" key={stringValue(backup.id)}>
                <div className="record-main">
                  <div className="record-title"><strong>{truncateId(backup.id)}</strong><StatusBadge tone={statusTone(backup.status)}>{statusLabel(backup.status)}</StatusBadge></div>
                  <p>{numberValue(manifest.file_count)} 个文件 · {formatBytes(manifest.total_bytes)}</p>
                  <small>完成：{formatTime(backup.completed_at)} · 验证：{formatTime(backup.verified_at)}</small>
                </div>
                <button className="secondary-button" type="button" disabled={busy} onClick={() => {
                  setSelected(stringValue(backup.id, ''));
                  void run(`/backups/${stringValue(backup.id, '')}/verify`, { method: 'POST' }, '备份验证任务已提交。');
                }}>重新验证</button>
              </article>
            );
          })}
        </div>
      )}

      <details className="advanced-section advanced-section--danger">
        <summary><span>高级：恢复与维护</span><small>会暂停数据面，请确认备份 ID 和管理员密码</small></summary>
        <div className="advanced-content">
          <Notice tone="danger">恢复会进入全局维护模式并替换 Vault、历史和 SQLite 状态。请先确认已有可用备份和独立主密钥。</Notice>
          <div className="form-grid">
            <label>备份<select value={selected} onChange={(event) => setSelected(event.target.value)}><option value="">请选择已完成备份</option>{backups.map((backup) => <option key={stringValue(backup.id)} value={stringValue(backup.id)}>{truncateId(backup.id)} · {statusLabel(backup.status)}</option>)}</select></label>
            <label>当前管理员密码<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" /></label>
          </div>
          <div className="button-row">
            <button className="secondary-button" type="button" disabled={busy || !selected} onClick={() => void run('/restore/validate', { method: 'POST', body: { backup_id: selected } }, '恢复包验证通过，尚未修改任何数据。')}>仅验证恢复包</button>
            <button className="danger-button" type="button" disabled={busy || !selected || password.length === 0} onClick={() => {
              if (window.confirm('确定开始恢复吗？数据面将进入维护模式。')) void run('/restore', { method: 'POST', body: { backup_id: selected, confirmation: 'RESTORE', password } }, '恢复任务已提交，数据面将进入维护模式。');
            }}>开始恢复</button>
            <button className="secondary-button" type="button" disabled={busy || password.length === 0} onClick={() => void run('/maintenance/recover', { method: 'POST', body: { confirmation: 'RECOVER', password } }, '维护恢复检查通过，服务正在重新开放。')}>执行 RECOVER 检查</button>
          </div>
        </div>
      </details>
    </Panel>
  );
}

function SystemPage({ data }: { data: JsonObject | null }) {
  const listeners = asRecord(data?.listeners);
  const database = asRecord(data?.database);
  const ready = booleanValue(data?.ready);
  return (
    <div className="panel-grid">
      <Panel title="服务运行状态" eyebrow="运行环境" actions={<StatusBadge tone={ready ? 'success' : 'warning'}>{ready ? '已就绪' : '未就绪'}</StatusBadge>}>
        <InfoGrid>
          <InfoItem label="版本" value={stringValue(data?.version)} mono />
          <InfoItem label="维护模式" value={statusLabel(data?.maintenance)} />
          <InfoItem label="数据监听" value={stringValue(listeners.data)} mono />
          <InfoItem label="管理监听" value={stringValue(listeners.admin)} mono />
          <InfoItem label="数据目录" value={stringValue(data?.data_dir)} mono />
          <InfoItem label="历史目录" value={stringValue(data?.history_root)} mono />
        </InfoGrid>
      </Panel>
      <Panel title="SQLite 状态" eyebrow="运行状态库" actions={<StatusBadge tone={booleanValue(database.integrity_ok) ? 'success' : 'danger'}>{booleanValue(database.integrity_ok) ? '完整性正常' : '需要检查'}</StatusBadge>}>
        <InfoGrid>
          <InfoItem label="迁移版本" value={numberValue(database.migration_version)} />
          <InfoItem label="外键错误" value={numberValue(database.foreign_key_violations)} />
          <InfoItem label="日志模式" value={stringValue(database.journal_mode)} mono />
          <InfoItem label="写入等待上限" value={`${numberValue(database.busy_timeout_ms)} ms`} />
        </InfoGrid>
      </Panel>
    </div>
  );
}

function Choice({ checked, label, detail, technical, onChange }: { checked: boolean; label: string; detail: string; technical?: string; onChange: () => void }) {
  return (
    <label className={`choice-card${checked ? ' choice-card--selected' : ''}`}>
      <input type="checkbox" checked={checked} onChange={onChange} />
      <span><strong>{label}</strong><small>{detail}</small>{technical ? <code>{technical}</code> : null}</span>
    </label>
  );
}

function SummaryRow({ label, value, mono = false }: { label: string; value: ReactNode; mono?: boolean }) {
  return <div className="summary-row"><span>{label}</span><strong className={mono ? 'mono-value' : undefined}>{value}</strong></div>;
}

function providerTypeLabel(value: unknown): string {
  const labels: Record<string, string> = {
    openai_compatible: 'OpenAI 兼容',
    openai_responses: 'OpenAI Responses',
    deepseek: 'DeepSeek',
    xiaomi_mimo: '小米 MiMo',
    zhipu_glm: '智谱 GLM',
    moonshot_kimi: 'Kimi / Moonshot',
    google_gemini: 'Google Gemini',
    alibaba_qwen: '阿里千问 / DashScope',
    anthropic_messages: 'Anthropic Messages',
    embedding_http: 'Embedding HTTP',
  };
  const providerType = stringValue(value);
  return labels[providerType] ?? providerType;
}

function modelCapabilityLabel(capabilities: JsonObject): string {
  const values = [];
  if (booleanValue(capabilities.structured_output)) values.push('结构化生成');
  if (booleanValue(capabilities.embeddings)) {
    const dimension = typeof capabilities.dimension === 'number' ? ` ${capabilities.dimension} 维` : '';
    values.push(`Embedding${dimension}`);
  }
  if (booleanValue(capabilities.reranking)) values.push('重排');
  return values.length > 0 ? values.join(' · ') : '能力待确认';
}

const openAiChatProviderTypes = new Set([
  'openai_compatible', 'deepseek', 'xiaomi_mimo', 'zhipu_glm',
  'moonshot_kimi', 'google_gemini', 'alibaba_qwen',
]);

function isOpenAiChatProvider(value: unknown): boolean {
  return openAiChatProviderTypes.has(stringValue(value, ''));
}

function detectProviderPreset(baseUrl: string): string {
  let host = '';
  try { host = new URL(baseUrl).hostname.toLowerCase(); } catch { return 'generic'; }
  if (host === 'api.deepseek.com') return 'deepseek';
  if (host === 'api.xiaomimimo.com') return 'xiaomi_mimo';
  if (host === 'open.bigmodel.cn') return 'zhipu_glm';
  if (host === 'api.moonshot.ai' || host === 'api.moonshot.cn') return 'moonshot_kimi';
  if (host === 'generativelanguage.googleapis.com') return 'google_gemini';
  if (host === 'dashscope.aliyuncs.com' || host === 'dashscope-intl.aliyuncs.com' || host.endsWith('.dashscope.aliyuncs.com') || host.endsWith('.maas.aliyuncs.com')) return 'alibaba_qwen';
  return 'generic';
}

function resolveCompatibilityPreset(configured: string, providerType: string, baseUrl: string): string {
  if (configured !== 'auto') return configured;
  if (providerType === 'openai_compatible') return detectProviderPreset(baseUrl);
  return openAiChatProviderTypes.has(providerType) ? providerType : 'generic';
}

function providerPresetSupportsThinking(preset: string): boolean {
  return ['deepseek', 'xiaomi_mimo', 'zhipu_glm', 'moonshot_kimi', 'google_gemini', 'alibaba_qwen'].includes(preset);
}

function openAiCompatibilityLabel(provider: JsonObject, model: JsonObject): string {
  const settings = asRecord(model.settings);
  const preset = resolveCompatibilityPreset(
    stringValue(settings.openai_compatibility_preset, 'auto'),
    stringValue(provider.provider_type, ''),
    stringValue(provider.base_url, ''),
  );
  const presetLabels: Record<string, string> = {
    generic: '通用 OpenAI', deepseek: 'DeepSeek', xiaomi_mimo: '小米 MiMo',
    zhipu_glm: '智谱 GLM', moonshot_kimi: 'Kimi', google_gemini: 'Gemini',
    alibaba_qwen: '千问',
  };
  const configuredOutput = stringValue(settings.openai_structured_output_mode, 'auto');
  const output = configuredOutput !== 'auto'
    ? configuredOutput
    : ['deepseek', 'xiaomi_mimo', 'zhipu_glm', 'alibaba_qwen'].includes(preset)
      ? 'json_object'
      : 'strict_json_schema';
  const outputLabels: Record<string, string> = {
    strict_json_schema: '严格 JSON Schema', json_object: 'JSON Object', prompt_only: '提示词 JSON',
  };
  const configuredThinking = stringValue(settings.openai_thinking_mode, 'auto');
  const thinking = configuredThinking === 'enabled'
    ? '思考开启'
    : configuredThinking === 'disabled'
      ? '思考关闭'
      : ['deepseek', 'xiaomi_mimo'].includes(preset) ? '思考开启' : '思考按模型默认';
  const defaultReasoningLimit = ['deepseek', 'xiaomi_mimo', 'moonshot_kimi', 'google_gemini', 'alibaba_qwen'].includes(preset);
  const configuredLimit = typeof settings.generation_token_limit === 'number'
    ? settings.generation_token_limit
    : defaultReasoningLimit ? 32_768 : null;
  const modelMaximum = asRecord(model.capabilities).max_output_tokens;
  const effectiveLimit = configuredLimit !== null && typeof modelMaximum === 'number'
    ? Math.min(configuredLimit, modelMaximum)
    : configuredLimit;
  const limit = effectiveLimit === null ? '按任务上限' : `单次上限 ${effectiveLimit}`;
  return `${presetLabels[preset] ?? preset} · ${outputLabels[output] ?? output} · ${thinking} · ${limit}`;
}

function jobTypeLabel(value: unknown): string {
  const labels: Record<string, string> = {
    'vault.reconcile': 'Vault 重新扫描',
    'index.rebuild': '重建知识索引',
    'outbox.event': '文件事件处理',
    'memory.extract': '阶段一：提取原始记忆',
    'memory.consolidate': '阶段二：整理长期记忆',
    'memory.reset_pipeline': '重置记忆系统',
    'memory.revalidate': '记忆来源校验',
    'memory.source_reconcile': '协调记忆来源',
    'memory.audit_sources': '审计记忆来源健康',
    'memory.rebuild': '重建记忆投影',
    'memory.repair_sources': '修复历史记忆来源',
    'embedding.rebuild': '重建语义向量',
    'backup.create': '创建备份',
    'backup.verify': '验证备份',
    'backup.restore': '恢复备份',
  };
  const jobType = stringValue(value);
  return labels[jobType] ?? jobType;
}

function auditActionLabel(value: unknown): string {
  const action = stringValue(value);
  const labels: Record<string, string> = {
    'admin.setup.completed': '完成首次初始化',
    'admin.login.succeeded': '管理员登录',
    'admin.login.failed': '管理员登录失败',
    'admin.vault.updated': '更新 Vault 设置',
    'admin.vault.rescan_queued': '提交 Vault 扫描',
    'admin.webdav_credential.issued': '创建 WebDAV 凭据',
    'admin.webdav_credential.revoked': '撤销 WebDAV 凭据',
    'admin.mcp_token.issued': '创建 MCP PAT',
    'admin.mcp_token.revoked': '撤销 MCP PAT',
    'admin.provider.created': '创建 AI 服务',
    'admin.provider.updated': '编辑 AI 服务',
    'admin.provider.deleted': '删除 AI 服务',
    'admin.provider.tested': '测试 AI 服务',
    'admin.provider_model.created': '登记 AI 模型',
    'admin.model_binding.updated': '更新模型用途',
    'admin.memory_extraction.updated': '更新自动记忆设置',
    'admin.memory_extraction.queued': '处理现有笔记',
    'admin.memory_extraction.restarted': '重置并重新处理自动记忆',
    'admin.memory.updated': '编辑长期记忆',
    'admin.memory.archived': '归档长期记忆',
    'admin.memory.restored': '恢复长期记忆',
    'admin.memory.deleted': '永久删除长期记忆',
    'admin.memory_source_audit.queued': '提交记忆来源健康审计',
    'admin.index.rebuild_queued': '提交索引重建',
    'admin.backup.created': '创建备份',
    'admin.restore.requested': '请求恢复',
  };
  return labels[action] ?? action;
}

function sameValues(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((value) => right.includes(value));
}

function vaultName(value: unknown): string {
  const name = stringValue(value, '默认 Vault');
  return name === 'Default Vault' ? '默认 Vault' : name;
}
