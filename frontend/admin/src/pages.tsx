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
          detail={`${numberValue(memory.candidate)} 条等待审核`}
        />
        <Metric label="后台任务" value={numberValue(jobs.pending)} detail="等待或正在执行" />
        <Metric label="索引覆盖率" value={formatPercent(index.coverage)} detail={`${numberValue(index.indexed_notes)} 篇已索引`} />
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

  return (
    <Panel
      title={vaultName(vault.name)}
      eyebrow="当前知识库"
      description="内容根目录是普通 Obsidian Vault，浏览器不能直接修改路径。"
      actions={<StatusBadge tone={statusTone(vault.status)}>{statusLabel(vault.status)}</StatusBadge>}
    >
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
  const [oauthOpen, setOauthOpen] = useState(false);

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
          <CopyField label="WebDAV 地址" value={stringValue(data?.webdav_endpoint)} />
        </div>
        <p className="muted compact-text">支持协议版本：{Array.isArray(data?.supported_mcp_revisions) ? data.supported_mcp_revisions.join('、') : '—'}</p>
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

      <details className="advanced-section" onToggle={(event) => setOauthOpen(event.currentTarget.open)}>
        <summary>
          <span>高级：OAuth 资源服务器</span>
          <small>仅在你已有外部 OAuth/OIDC 服务时配置</small>
        </summary>
        {oauthOpen ? <OAuthForms notify={notify} /> : null}
      </details>
    </div>
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

function OAuthForms({ notify }: { notify: Notify }) {
  const [issuerValues, setIssuerValues] = useState({ name: '', issuer_url: '', audience: 'mcp-vault', resource: '', jwks_cache_json: '' });
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
      <Notice tone="warning">只接受 RS256 的 RSA 公钥 JWKS。不要粘贴私钥、客户端密钥或对称密钥。</Notice>
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
          <label>资源 URL<input required type="url" value={issuerValues.resource} onChange={(event) => setIssuerValues({ ...issuerValues, resource: event.target.value })} /></label>
          <label className="form-span">RSA 公钥 JWKS<textarea required rows={6} value={issuerValues.jwks_cache_json} onChange={(event) => setIssuerValues({ ...issuerValues, jwks_cache_json: event.target.value })} placeholder='{"keys":[{"kty":"RSA","kid":"…","alg":"RS256","n":"…","e":"AQAB"}]}' /></label>
        </div>
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

function ProviderPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const providers = arrayRecords(data?.providers);

  async function testProvider(provider: JsonObject) {
    try {
      await adminApi.request(`/providers/${stringValue(provider.id, '')}/test`, { method: 'POST' });
      notify('连接测试已完成，健康状态会在刷新后显示。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
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
              return (
                <article className="record-item" key={stringValue(provider.id)}>
                  <div className="record-main">
                    <div className="record-title"><strong>{stringValue(provider.name)}</strong><StatusBadge tone={statusTone(health.status)}>{statusLabel(health.status)}</StatusBadge></div>
                    <p>{providerTypeLabel(provider.provider_type)} · <code>{stringValue(provider.base_url)}</code></p>
                    <small>{booleanValue(secret.configured) ? `密钥：${stringValue(secret.hint, '已配置')}` : '未配置密钥'} · 最近检查：{formatTime(health.checked_at)}</small>
                  </div>
                  <button className="secondary-button" type="button" onClick={() => void testProvider(provider)}>测试连接</button>
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
    </div>
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

function AddProviderForm({ notify, onRefresh }: { notify: Notify; onRefresh: () => void }) {
  const [values, setValues] = useState({ name: '', provider_type: 'openai_compatible', base_url: '', secret: '' });
  const [busy, setBusy] = useState(false);

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
        <label>接口类型<select value={values.provider_type} onChange={(event) => setValues({ ...values, provider_type: event.target.value })}><option value="openai_compatible">OpenAI 兼容</option><option value="openai_responses">OpenAI Responses</option><option value="anthropic_messages">Anthropic Messages</option><option value="embedding_http">Embedding HTTP</option></select></label>
        <label>Base URL<input required type="url" value={values.base_url} onChange={(event) => setValues({ ...values, base_url: event.target.value })} placeholder="https://provider.example/v1/" /></label>
        <label>API 密钥（可选）<input type="password" value={values.secret} onChange={(event) => setValues({ ...values, secret: event.target.value })} autoComplete="new-password" /></label>
      </div>
      <button className="primary-button" disabled={busy} type="submit">{busy ? '正在保存…' : '安全保存 AI 服务'}</button>
    </form>
  );
}

function IndexPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const status = asRecord(data?.status);
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
            <Metric label="覆盖率" value={formatPercent(status.coverage)} detail="已处理的可索引内容" />
            <Metric label="Markdown" value={numberValue(status.indexed_notes)} detail="已索引笔记" />
            <Metric label="全部条目" value={numberValue(status.indexed_entries)} detail="包括附件元数据" />
            <Metric label="索引内容" value={formatBytes(status.indexed_bytes)} detail={`分析器 ${stringValue(status.analyzer_version)}`} />
          </section>
          <p className="muted compact-text">上次完成：{formatTime(status.last_rebuilt_at)} · 索引版本：{numberValue(status.revision)}</p>
          {typeof status.last_error === 'string' && status.last_error ? <Notice tone="danger">最近错误：{status.last_error}</Notice> : null}
        </>
      )}
    </Panel>
  );
}

function MemoryPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const memories = arrayRecords(data?.memories);
  const candidates = arrayRecords(data?.candidates);

  async function review(candidate: JsonObject, action: 'promote' | 'reject') {
    try {
      await adminApi.request(`/memory-candidates/${stringValue(candidate.id, '')}/${action}`, {
        method: 'POST',
        body: action === 'reject' ? { reason: '管理员在控制台拒绝' } : undefined,
      });
      notify(action === 'promote' ? '候选记忆已提升并写入规范 Markdown。' : '候选记忆已拒绝。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    }
  }

  return (
    <div className="page-stack">
      <Notice tone="info">生效记忆会物化为 Vault 中的 Markdown；数据库、全文索引和向量只是可重建投影。</Notice>
      <Panel title={`待审核候选（${candidates.length}）`} eyebrow="需要判断" description="模型输出只是提议，提升前请检查内容和来源。">
        {candidates.length === 0 ? (
          <EmptyState title="没有待审核候选" detail="自动提取的新候选会出现在这里。" />
        ) : (
          <div className="record-list">
            {candidates.map((candidate) => {
              const proposal = asRecord(candidate.candidate);
              return (
                <article className="record-item record-item--stack" key={stringValue(candidate.id)}>
                  <div className="record-title"><strong>{stringValue(proposal.content, '未提供候选内容')}</strong><StatusBadge tone="warning">待审核</StatusBadge></div>
                  <p>来源：<code>{stringValue(candidate.source_path)}</code></p>
                  <small>置信度 {formatPercent(candidate.confidence)} · 重要度 {formatPercent(candidate.importance)} · {formatTime(candidate.created_at)}</small>
                  <div className="button-row">
                    <button className="primary-button" type="button" onClick={() => void review(candidate, 'promote')}>提升为长期记忆</button>
                    <button className="secondary-button" type="button" onClick={() => void review(candidate, 'reject')}>拒绝</button>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </Panel>
      <Panel title={`长期记忆（${memories.length}）`} eyebrow="有来源的上下文" description="默认召回不调用在线模型。">
        {memories.length === 0 ? (
          <EmptyState title="还没有长期记忆" detail="Agent 的 remember 或审核通过的候选会出现在这里。" />
        ) : (
          <div className="record-list">
            {memories.map((memory) => (
              <article className="record-item record-item--stack" key={stringValue(memory.id)}>
                <div className="record-title"><strong>{stringValue(memory.content, '无内容')}</strong><StatusBadge tone={statusTone(memory.status)}>{statusLabel(memory.status)}</StatusBadge></div>
                <p>{memoryTypeLabel(memory.memory_type)} · <code>{stringValue(memory.canonical_path)}</code></p>
                <small>置信度 {formatPercent(memory.confidence)} · 重要度 {formatPercent(memory.importance)} · 最近更新 {formatTime(memory.updated_at)}</small>
              </article>
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}

function JobsPage({ data, notify, onRefresh }: { data: JsonObject | null; notify: Notify; onRefresh: () => void }) {
  const jobs = arrayRecords(data?.jobs);

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

  return (
    <Panel title={`最近任务（${jobs.length}）`} eyebrow="持久化队列" description="服务重启后任务仍可恢复，不依赖内存中的临时线程。">
      {jobs.length === 0 ? (
        <EmptyState title="当前没有任务" detail="扫描、索引、备份和记忆处理任务会显示在这里。" />
      ) : (
        <div className="record-list">
          {jobs.map((job) => {
            const status = stringValue(job.status, 'unknown');
            const cancellable = ['queued', 'running', 'retry_wait'].includes(status);
            return (
              <article className="record-item" key={stringValue(job.id)}>
                <div className="record-main">
                  <div className="record-title"><strong>{jobTypeLabel(job.job_type)}</strong><StatusBadge tone={statusTone(status)}>{statusLabel(status)}</StatusBadge></div>
                  <p><code>{truncateId(job.id)}</code> · 进度 {formatPercent(job.progress)}</p>
                  <small>尝试 {numberValue(job.attempts)} / {numberValue(job.max_attempts)} · 更新于 {formatTime(job.updated_at)}{typeof job.last_error === 'string' && job.last_error ? ` · ${job.last_error}` : ''}</small>
                </div>
                <div className="button-column">
                  {status === 'failed' ? <button className="secondary-button" type="button" onClick={() => void act(job, 'retry')}>重试</button> : null}
                  {cancellable ? <button className="danger-link" type="button" onClick={() => void act(job, 'cancel')}>取消</button> : null}
                </div>
              </article>
            );
          })}
        </div>
      )}
    </Panel>
  );
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
    anthropic_messages: 'Anthropic Messages',
    embedding_http: 'Embedding HTTP',
  };
  const providerType = stringValue(value);
  return labels[providerType] ?? providerType;
}

function jobTypeLabel(value: unknown): string {
  const labels: Record<string, string> = {
    'vault.reconcile': 'Vault 重新扫描',
    'index.rebuild': '重建知识索引',
    'outbox.event': '文件事件处理',
    'memory.extract': '记忆提取',
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
    'admin.provider.tested': '测试 AI 服务',
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
