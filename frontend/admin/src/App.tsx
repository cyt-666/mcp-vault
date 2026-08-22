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

const pageEndpoints: Partial<Record<Page, string>> = {
  dashboard: '/dashboard',
  vault: '/vault',
  webdav: '/webdav/credentials',
  providers: '/providers',
  index: '/index/status',
  jobs: '/jobs?limit=50',
  audit: '/audit?limit=50',
  backup: '/backups?limit=50',
  system: '/system',
};

async function loadPage(page: Page): Promise<JsonObject> {
  if (page === 'webdav') {
    const [credentialData, connection] = await Promise.all([
      adminApi.request<{ credentials: unknown[] }>('/webdav/credentials'),
      adminApi.request<JsonObject>('/mcp/connection-info'),
    ]);
    return { credentials: credentialData.credentials, webdav_endpoint: connection.webdav_endpoint };
  }

  if (page === 'mcp') {
    const [connection, tokenData] = await Promise.all([
      adminApi.request<JsonObject>('/mcp/connection-info'),
      adminApi.request<{ tokens: unknown[] }>('/mcp/tokens?limit=50'),
    ]);
    return { ...connection, tokens: tokenData.tokens };
  }

  if (page === 'memory') {
    const [memoryData, candidateData] = await Promise.all([
      adminApi.request<JsonObject>('/memories?limit=50'),
      adminApi.request<{ candidates: unknown[] }>('/memory-candidates?limit=50'),
    ]);
    return { ...memoryData, candidates: candidateData.candidates };
  }

  const endpoint = pageEndpoints[page];
  return endpoint ? adminApi.request<JsonObject>(endpoint) : {};
}

export function App() {
  const [authenticated, setAuthenticated] = useState(false);
  const [setupAvailable, setSetupAvailable] = useState<boolean | null>(null);
  const [setupStatusRevision, setSetupStatusRevision] = useState(0);
  const [page, setPage] = useState<Page>('dashboard');
  const [data, setData] = useState<JsonObject | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshRevision, setRefreshRevision] = useState(0);

  useEffect(() => {
    document.documentElement.scrollTop = 0;
    document.body.scrollTop = 0;
  }, [authenticated, page]);

  useEffect(() => {
    if (authenticated) return;
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
  }, [authenticated, setupStatusRevision]);

  useEffect(() => {
    if (!authenticated) return;
    let cancelled = false;
    setLoading(true);
    setError(null);

    loadPage(page)
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
  }, [authenticated, page, refreshRevision]);

  function navigate(nextPage: Page) {
    setPage(nextPage);
    setData(null);
    setError(null);
  }

  if (!authenticated) {
    return (
      <main className="auth-shell">
        <section className="auth-brand" aria-labelledby="product-title">
          <div className="brand-mark" aria-hidden="true">
            MV
          </div>
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
          checking={setupAvailable === null}
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
          <span className="sidebar-logo" aria-hidden="true">
            MV
          </span>
          <div>
            <strong>MCP Vault</strong>
            <span>管理控制台</span>
          </div>
        </div>
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
            <p className="breadcrumb">MCP Vault / {selected.label}</p>
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
          <Dashboard data={data} onNavigate={navigate} />
        ) : (
          <ManagementPage
            page={page}
            data={data}
            onRefresh={() => setRefreshRevision((revision) => revision + 1)}
          />
        )}
      </section>
    </main>
  );
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
