import { useEffect, useState } from 'react';
import type { FormEvent } from 'react';

import { AdminApiError, adminApi } from './api';
import { Dashboard, ManagementPage } from './pages';
import { InlineAlert, LoadingBar, PasswordPolicyHelp } from './ui';
import {
  type JsonObject,
  type Page,
  formatRequestError,
  navigationGroups,
  pageMeta,
} from './view-model';

type VaultSummary = {
  id: string;
  slug: string;
  name: string;
  status: string;
  availability: string;
  content_root: string;
};

const globalPageEndpoints: Partial<Record<Page, string>> = {
  backup: '/backups?limit=50',
  system: '/system',
};

export function isInsecureLanAdminLocation(location: Pick<Location, 'protocol' | 'hostname'>) {
  return location.protocol === 'http:'
    && !['localhost', '127.0.0.1', '[::1]'].includes(location.hostname);
}

function scopedPath(vaultSlug: string, path = ''): string {
  return `/vaults/${encodeURIComponent(vaultSlug)}${path}`;
}

async function loadPage(page: Page, vaultSlug: string): Promise<JsonObject> {
  if (page === 'webdav') {
    const [credentialData, connection] = await Promise.all([
      adminApi.request<{ credentials: unknown[] }>(scopedPath(vaultSlug, '/webdav/credentials')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/mcp/connection-info')),
    ]);
    return { credentials: credentialData.credentials, webdav_endpoint: connection.webdav_endpoint };
  }

  if (page === 'mcp') {
    const [connection, tokenData, localOAuth] = await Promise.all([
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/mcp/connection-info')),
      adminApi.request<{ tokens: unknown[] }>(scopedPath(vaultSlug, '/mcp/tokens?limit=50')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/mcp/oauth/local')),
    ]);
    return { ...connection, tokens: tokenData.tokens, local_oauth: localOAuth };
  }

  if (page === 'providers') {
    const [providerData, bindingData] = await Promise.all([
      adminApi.request<{ providers: unknown[]; provider_mode: unknown }>(scopedPath(vaultSlug, '/providers')),
      adminApi.request<{ bindings: unknown[] }>(scopedPath(vaultSlug, '/model-bindings')),
    ]);
    const providers = Array.isArray(providerData.providers) ? providerData.providers : [];
    const modelGroups = await Promise.all(
      providers.map(async (provider) => {
        const id = typeof provider === 'object' && provider !== null && 'id' in provider
          ? String(provider.id)
          : '';
        if (!id) return [];
        const result = await adminApi.request<{ models: unknown[] }>(`/providers/${encodeURIComponent(id)}/models`);
        const providerName = typeof provider === 'object' && provider !== null && 'name' in provider
          ? String(provider.name)
          : id;
        return result.models.map((model) => (
          typeof model === 'object' && model !== null
            ? { ...model, provider_name: providerName }
            : model
        ));
      }),
    );
    return {
      ...providerData,
      bindings: bindingData.bindings,
      models: modelGroups.flat(),
    };
  }

  if (page === 'memory') {
    const [memoryData, extractionData, sourceHealthData, retrievalData, embeddingData, jobsOverview] = await Promise.all([
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/memories?limit=50')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/memory/extraction')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/memory/source-health?limit=50')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/memory/retrieval')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/memory/embeddings')),
      adminApi.request<JsonObject>(scopedPath(vaultSlug, '/jobs/overview?limit=50')),
    ]);
    const memoryJobs = ['running', 'queued', 'retry_wait', 'history']
      .flatMap((group) => Array.isArray(jobsOverview[group]) ? jobsOverview[group] : [])
      .filter((job) => (
        typeof job === 'object'
        && job !== null
        && 'job_type' in job
        && String(job.job_type).startsWith('memory.')
      ));
    return {
      ...memoryData,
      extraction: extractionData,
      source_health: sourceHealthData,
      retrieval: retrievalData,
      embedding: embeddingData,
      memory_jobs: memoryJobs,
    };
  }

  const endpoint = globalPageEndpoints[page] ?? ({
    dashboard: scopedPath(vaultSlug, '/dashboard'),
    vault: scopedPath(vaultSlug),
    index: scopedPath(vaultSlug, '/index/status'),
    jobs: scopedPath(vaultSlug, '/jobs/overview?limit=50'),
    audit: scopedPath(vaultSlug, '/audit?limit=50'),
  } as Partial<Record<Page, string>>)[page];
  return endpoint ? adminApi.request<JsonObject>(endpoint) : {};
}

export function App() {
  const [authenticated, setAuthenticated] = useState(false);
  const [sessionChecked, setSessionChecked] = useState(false);
  const [setupAvailable, setSetupAvailable] = useState<boolean | null>(null);
  const [setupStatusRevision, setSetupStatusRevision] = useState(0);
  const [page, setPage] = useState<Page>('dashboard');
  const [data, setData] = useState<JsonObject | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshRevision, setRefreshRevision] = useState(0);
  const [vaults, setVaults] = useState<VaultSummary[]>([]);
  const [selectedVaultSlug, setSelectedVaultSlug] = useState('');
  const [vaultRevision, setVaultRevision] = useState(0);

  useEffect(() => {
    document.documentElement.scrollTop = 0;
    document.body.scrollTop = 0;
  }, [authenticated, page]);

  useEffect(() => {
    let cancelled = false;
    adminApi
      .restoreSession()
      .then((session) => {
        if (!cancelled) setAuthenticated(session !== null);
      })
      .catch((requestError: unknown) => {
        if (!cancelled) setError(formatRequestError(requestError));
      })
      .finally(() => {
        if (!cancelled) setSessionChecked(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!sessionChecked || authenticated) return;
    let cancelled = false;
    setSetupAvailable(null);

    adminApi
      .setupStatus()
      .then((status) => {
        if (!cancelled) setSetupAvailable(status.setup_available);
      })
      .catch(() => {
        if (cancelled) return;
        setSetupAvailable(false);
        setError('无法确认系统是否尚未初始化。为避免误开放注册，当前仅显示管理员登录。');
      });

    return () => {
      cancelled = true;
    };
  }, [authenticated, sessionChecked, setupStatusRevision]);

  useEffect(() => {
    if (!authenticated) return;
    let cancelled = false;
    adminApi
      .request<{ vaults: VaultSummary[] }>('/vaults')
      .then((result) => {
        if (cancelled) return;
        const nextVaults = Array.isArray(result.vaults) ? result.vaults : [];
        setVaults(nextVaults);
        setSelectedVaultSlug((current) => {
          if (nextVaults.some((vault) => vault.slug === current)) return current;
          const requested = new URLSearchParams(window.location.search).get('vault');
          if (requested && nextVaults.some((vault) => vault.slug === requested)) return requested;
          return nextVaults.find((vault) => vault.slug === 'default')?.slug
            ?? nextVaults.find((vault) => vault.status === 'active')?.slug
            ?? nextVaults[0]?.slug
            ?? '';
        });
      })
      .catch((requestError: unknown) => {
        if (!cancelled) setError(formatRequestError(requestError));
      });
    return () => {
      cancelled = true;
    };
  }, [authenticated, vaultRevision]);

  useEffect(() => {
    adminApi.setVaultSlug(selectedVaultSlug || null);
    if (!selectedVaultSlug) return;
    const url = new URL(window.location.href);
    url.searchParams.set('vault', selectedVaultSlug);
    window.history.replaceState(null, '', url);
    setData(null);
    setError(null);
  }, [selectedVaultSlug]);

  useEffect(() => {
    if (!authenticated || !selectedVaultSlug) return;
    let cancelled = false;
    setLoading(true);
    setError(null);

    loadPage(page, selectedVaultSlug)
      .then((result) => {
        if (!cancelled) setData(result);
      })
      .catch((requestError: unknown) => {
        if (cancelled) return;
        if (requestError instanceof Error && 'status' in requestError && requestError.status === 401) {
          setAuthenticated(false);
          adminApi.clearSession();
        }
        setError(formatRequestError(requestError));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [authenticated, page, refreshRevision, selectedVaultSlug]);

  useEffect(() => {
    if (!authenticated || !['jobs', 'memory'].includes(page)) return;
    const memoryHasActiveJob = page === 'memory'
      && Array.isArray(data?.memory_jobs)
      && data.memory_jobs.some((job) => (
        typeof job === 'object'
        && job !== null
        && 'status' in job
        && ['queued', 'running', 'retry_wait'].includes(String(job.status))
      ));
    if (page === 'memory' && !memoryHasActiveJob) return;
    const timer = window.setInterval(
      () => setRefreshRevision((revision) => revision + 1),
      5_000,
    );
    return () => window.clearInterval(timer);
  }, [authenticated, data, page]);

  function navigate(nextPage: Page) {
    setPage(nextPage);
    setData(null);
    setError(null);
  }

  if (!authenticated) {
    return (
      <main className="auth-shell">
        <section className="auth-brand" aria-labelledby="product-title">
          <img className="brand-mark" src="/mcp-vault-logo.png" alt="" aria-hidden="true" />
          <p className="eyebrow">MCP VAULT 管理端</p>
          <h1 id="product-title">你的知识库，始终由你掌控。</h1>
          <p className="lede">
            在一个地方管理 Obsidian 同步、Agent 访问、长期记忆和备份。Markdown 原文件始终属于你。
          </p>
          <div className="feature-tags" aria-label="核心能力">
            <span>Markdown 原文件</span>
            <span>WebDAV 同步</span>
            <span>MCP Agent</span>
          </div>
        </section>
        <AuthCard
          mode={setupAvailable === true ? 'setup' : 'login'}
          checking={!sessionChecked || setupAvailable === null}
          onAuthenticated={() => {
            setAuthenticated(true);
            setPage('dashboard');
          }}
          onSetupRejected={() => {
            setSetupAvailable(null);
            setSetupStatusRevision((revision) => revision + 1);
          }}
          onError={setError}
          error={error}
        />
      </main>
    );
  }

  const selected = pageMeta[page];
  return (
    <main className="app-shell">
      <aside className="sidebar" aria-label="管理端导航">
        <div className="sidebar-brand">
          <img className="sidebar-logo" src="/mcp-vault-logo.png" alt="" aria-hidden="true" />
          <div>
            <strong>MCP Vault</strong>
            <span>管理控制台</span>
          </div>
        </div>
        <VaultSwitcher
          vaults={vaults}
          selected={selectedVaultSlug}
          onSelect={setSelectedVaultSlug}
          onCreated={(slug) => {
            setSelectedVaultSlug(slug);
            setVaultRevision((revision) => revision + 1);
          }}
          onError={setError}
        />
        <nav>
          {navigationGroups.map((group) => (
            <section className="nav-group" key={group.label} aria-label={group.label}>
              <p>{group.label}</p>
              {group.pages.map((item) => {
                const meta = pageMeta[item];
                return (
                  <button
                    className={`nav-item${page === item ? ' nav-item--active' : ''}`}
                    key={item}
                    type="button"
                    aria-current={page === item ? 'page' : undefined}
                    onClick={() => navigate(item)}
                  >
                    <span className="nav-icon" aria-hidden="true">
                      {meta.icon}
                    </span>
                    <span>{meta.shortLabel}</span>
                  </button>
                );
              })}
            </section>
          ))}
        </nav>
        <div className="sidebar-footer">
          <span className="session-indicator">
            <span aria-hidden="true" />管理会话已连接
          </span>
          <button
            className="signout-button"
            type="button"
            onClick={() => {
              void adminApi.logout().finally(() => {
                setAuthenticated(false);
                setData(null);
              });
            }}
          >
            退出登录
          </button>
        </div>
      </aside>

      <section className="content-shell">
        <header className="content-header">
          <div>
            <p className="breadcrumb">
              MCP Vault / {vaults.find((vault) => vault.slug === selectedVaultSlug)?.name ?? selectedVaultSlug} / {selected.label}
            </p>
            <h1>{selected.label}</h1>
            <p>{selected.description}</p>
          </div>
          <button
            className="refresh-button"
            type="button"
            disabled={loading}
            onClick={() => setRefreshRevision((revision) => revision + 1)}
          >
            {loading ? '正在刷新' : '刷新状态'}
          </button>
        </header>

        {error ? <InlineAlert message={error} onDismiss={() => setError(null)} /> : null}
        {loading ? <LoadingBar /> : null}

        {page === 'dashboard' ? (
          <Dashboard key={selectedVaultSlug} data={data} onNavigate={navigate} />
        ) : (
          <ManagementPage
            key={`${selectedVaultSlug}:${page}`}
            page={page}
            data={data}
            onRefresh={() => setRefreshRevision((revision) => revision + 1)}
          />
        )}
      </section>
    </main>
  );
}

function VaultSwitcher({
  vaults,
  selected,
  onSelect,
  onCreated,
  onError,
}: {
  vaults: VaultSummary[];
  selected: string;
  onSelect: (slug: string) => void;
  onCreated: (slug: string) => void;
  onError: (message: string | null) => void;
}) {
  const [name, setName] = useState('');
  const [slug, setSlug] = useState('');
  const [busy, setBusy] = useState(false);

  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    onError(null);
    try {
      const result = await adminApi.request<{ vault: VaultSummary }>('/vaults', {
        method: 'POST',
        body: { name, slug },
      });
      setName('');
      setSlug('');
      onCreated(result.vault.slug);
    } catch (requestError: unknown) {
      onError(formatRequestError(requestError));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="vault-switcher" aria-label="当前 Vault">
      <label>
        当前 Vault
        <select
          value={selected}
          onChange={(event) => onSelect(event.target.value)}
          disabled={vaults.length === 0}
        >
          {vaults.map((vault) => (
            <option key={vault.id} value={vault.slug}>
              {vault.name} · {vaultAvailabilityLabel(vault.availability)}
            </option>
          ))}
        </select>
      </label>
      <details>
        <summary>新建 Vault</summary>
        <form className="vault-create-form" onSubmit={create}>
          <label>
            显示名称
            <input value={name} onChange={(event) => setName(event.target.value)} required maxLength={256} />
          </label>
          <label>
            链接标识
            <input
              value={slug}
              onChange={(event) => setSlug(event.target.value.toLowerCase())}
              required
              maxLength={64}
              pattern="[a-z0-9](?:[a-z0-9-]*[a-z0-9])?"
              placeholder="work"
              autoCapitalize="none"
              spellCheck={false}
            />
          </label>
          <small>内容目录由服务创建；标识将进入 WebDAV 和 MCP 链接，创建后不可修改。</small>
          <button className="secondary-button" type="submit" disabled={busy}>
            {busy ? '正在创建…' : '创建 Vault'}
          </button>
        </form>
      </details>
    </section>
  );
}

function vaultAvailabilityLabel(value: string): string {
  return ({
    initializing: '初始化中',
    ready: '就绪',
    maintenance: '维护中',
    disabled: '已停用',
    error: '异常',
  } as Record<string, string>)[value] ?? value;
}

function AuthCard({
  mode,
  checking,
  onAuthenticated,
  onSetupRejected,
  onError,
  error,
}: {
  mode: 'login' | 'setup';
  checking: boolean;
  onAuthenticated: () => void;
  onSetupRejected: () => void;
  onError: (message: string | null) => void;
  error: string | null;
}) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    onError(null);
    try {
      if (mode === 'setup') await adminApi.setup(username, password);
      await adminApi.login(username, password);
      onAuthenticated();
    } catch (requestError: unknown) {
      if (
        mode === 'setup' &&
        requestError instanceof AdminApiError &&
        requestError.code === 'setup_unavailable'
      ) {
        onSetupRejected();
      }
      onError(formatRequestError(requestError));
    } finally {
      setBusy(false);
    }
  }

  if (checking) {
    return (
      <section className="auth-card" aria-labelledby="auth-title" aria-live="polite">
        <p className="auth-mode-label">管理端状态</p>
        <h2 id="auth-title">正在检查初始化状态</h2>
        <p className="muted">确认当前应显示首次初始化还是管理员登录。</p>
        <LoadingBar />
      </section>
    );
  }

  return (
    <section className="auth-card" aria-labelledby="auth-title">
      <p className="auth-mode-label">{mode === 'login' ? '管理员登录' : '首次初始化'}</p>
      <h2 id="auth-title">{mode === 'login' ? '欢迎回来' : '创建第一个管理员'}</h2>
      <p className="muted">
        {mode === 'login'
          ? '使用管理端账号登录。WebDAV 密码和 MCP Token 是彼此独立的凭据。'
          : '设置管理员账号和密码即可完成初始化。该账号将拥有 MCP Vault 的管理权限。'}
      </p>
      {isInsecureLanAdminLocation(window.location) ? (
        <p className="transport-warning" role="status">
          当前通过局域网 HTTP 访问，管理员密码和会话不会经过传输加密。仅应在可信局域网内使用。
        </p>
      ) : null}
      {error ? <InlineAlert message={error} /> : null}
      <form onSubmit={submit} className="form-stack">
        <label>
          用户名
          <input
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            required
            autoComplete="username"
            placeholder="管理员用户名"
          />
        </label>
        <label>
          密码
          <input
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
            type="password"
            autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
            aria-describedby={mode === 'setup' ? 'admin-password-policy' : undefined}
            placeholder={mode === 'login' ? '输入管理端密码' : '设置一个较长的密码'}
          />
          {mode === 'setup' ? <PasswordPolicyHelp id="admin-password-policy" /> : null}
        </label>
        <button className="primary-button primary-button--wide" type="submit" disabled={busy}>
          {busy ? '请稍候…' : mode === 'login' ? '登录管理端' : '完成初始化并登录'}
        </button>
      </form>
    </section>
  );
}
