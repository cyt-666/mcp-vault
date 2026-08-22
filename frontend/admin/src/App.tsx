import { useEffect, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';

import { AdminApiError, adminApi } from './api';

type Page =
  | 'dashboard'
  | 'vault'
  | 'webdav'
  | 'mcp'
  | 'providers'
  | 'index'
  | 'memory'
  | 'jobs'
  | 'audit'
  | 'backup'
  | 'system';

type JsonObject = Record<string, unknown>;

const navigation: Array<{ id: Page; label: string; hint: string }> = [
  { id: 'dashboard', label: 'Dashboard', hint: 'Service health at a glance' },
  { id: 'vault', label: 'Vault', hint: 'Storage and scan policy' },
  { id: 'webdav', label: 'WebDAV', hint: 'Device credentials' },
  { id: 'mcp', label: 'MCP Access', hint: 'Tokens and OAuth resource server' },
  { id: 'providers', label: 'AI Providers', hint: 'Models, roles, and privacy' },
  { id: 'index', label: 'Knowledge Index', hint: 'FTS and knowledge map' },
  { id: 'memory', label: 'Memory', hint: 'Provenance and candidates' },
  { id: 'jobs', label: 'Jobs', hint: 'Durable work and retries' },
  { id: 'audit', label: 'Audit', hint: 'Redacted security history' },
  { id: 'backup', label: 'Backup & Restore', hint: 'Verified recovery workflows' },
  { id: 'system', label: 'System', hint: 'Runtime and migration details' },
];

const pageEndpoints: Partial<Record<Page, string>> = {
  dashboard: '/dashboard',
  vault: '/vault',
  webdav: '/webdav/credentials',
  mcp: '/mcp/connection-info',
  providers: '/providers',
  index: '/index/status',
  memory: '/memories?limit=50',
  jobs: '/jobs?limit=50',
  audit: '/audit?limit=50',
  backup: '/backups?limit=50',
  system: '/system',
};

export function App() {
  const [authenticated, setAuthenticated] = useState(false);
  const [authMode, setAuthMode] = useState<'login' | 'setup'>('login');
  const [page, setPage] = useState<Page>('dashboard');
  const [data, setData] = useState<JsonObject | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!authenticated || !pageEndpoints[page]) {
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    adminApi
      .request<JsonObject>(pageEndpoints[page] ?? '')
      .then((result) => {
        if (!cancelled) {
          setData(result);
        }
      })
      .catch((requestError: unknown) => {
        if (!cancelled) {
          if (requestError instanceof AdminApiError && requestError.status === 401) {
            setAuthenticated(false);
            adminApi.clearSession();
          }
          setError(requestError instanceof Error ? requestError.message : 'The request failed.');
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [authenticated, page]);

  if (!authenticated) {
    return (
      <main className="auth-shell">
        <div className="auth-brand">
          <p className="eyebrow">MCP VAULT / CONTROL PLANE</p>
          <h1>Keep the Vault human-owned.</h1>
          <p className="lede">
            The Admin console is separate from the public MCP and WebDAV data plane. It is available
            only on the trusted control listener.
          </p>
          <div className="plane-tags" aria-label="Service planes">
            <span>Data plane</span>
            <span>Control plane</span>
          </div>
        </div>
        <AuthCard
          mode={authMode}
          onModeChange={setAuthMode}
          onAuthenticated={() => setAuthenticated(true)}
          onError={setError}
          error={error}
        />
      </main>
    );
  }

  const selected = navigation.find((item) => item.id === page) ?? navigation[0];
  return (
    <main className="app-shell">
      <aside className="sidebar" aria-label="Admin navigation">
        <div className="sidebar-brand">
          <p className="eyebrow">MCP VAULT</p>
          <strong>Control plane</strong>
          <span>Private by design</span>
        </div>
        <nav>
          {navigation.map((item) => (
            <button
              className={`nav-item${page === item.id ? ' nav-item--active' : ''}`}
              key={item.id}
              type="button"
              aria-current={page === item.id ? 'page' : undefined}
              onClick={() => {
                setPage(item.id);
                setData(null);
              }}
            >
              <span>{item.label}</span>
              <small>{item.hint}</small>
            </button>
          ))}
        </nav>
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
          Sign out
        </button>
      </aside>
      <section className="content-shell">
        <header className="content-header">
          <div>
            <p className="eyebrow">ADMIN / {selected.label.toUpperCase()}</p>
            <h1>{selected.label}</h1>
            <p className="lede">{selected.hint}. Changes are validated by the server and recorded for audit.</p>
          </div>
          <span className="private-badge">LAN / VPN only</span>
        </header>
        {error ? <InlineAlert message={error} onDismiss={() => setError(null)} /> : null}
        {loading ? <div className="loading-bar" role="status">Loading current control-plane state…</div> : null}
        {page === 'dashboard' ? <Dashboard data={data} /> : <ManagementPage page={page} data={data} />}
      </section>
    </main>
  );
}

function AuthCard({
  mode,
  onModeChange,
  onAuthenticated,
  onError,
  error,
}: {
  mode: 'login' | 'setup';
  onModeChange: (mode: 'login' | 'setup') => void;
  onAuthenticated: () => void;
  onError: (message: string | null) => void;
  error: string | null;
}) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [bootstrapToken, setBootstrapToken] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    onError(null);
    try {
      if (mode === 'setup') {
        await adminApi.setup(bootstrapToken, username, password);
      }
      await adminApi.login(username, password);
      onAuthenticated();
    } catch (requestError: unknown) {
      onError(requestError instanceof Error ? requestError.message : 'Authentication failed.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="auth-card" aria-labelledby="auth-title">
      <div className="auth-tabs" role="tablist" aria-label="Admin authentication">
        <button type="button" role="tab" aria-selected={mode === 'login'} onClick={() => onModeChange('login')}>
          Sign in
        </button>
        <button type="button" role="tab" aria-selected={mode === 'setup'} onClick={() => onModeChange('setup')}>
          First-run setup
        </button>
      </div>
      <h2 id="auth-title">{mode === 'login' ? 'Welcome back' : 'Create the first Admin'}</h2>
      <p className="muted">
        {mode === 'login'
          ? 'Use the Admin password. WebDAV passwords and MCP tokens are separate credentials.'
          : 'Setup is one-time and requires the bootstrap token retrieved locally from MCP Vault.'}
      </p>
      {error ? <InlineAlert message={error} /> : null}
      <form onSubmit={submit} className="form-stack">
        {mode === 'setup' ? (
          <label>
            Bootstrap token
            <input value={bootstrapToken} onChange={(event) => setBootstrapToken(event.target.value)} required type="password" autoComplete="off" />
          </label>
        ) : null}
        <label>
          Username
          <input value={username} onChange={(event) => setUsername(event.target.value)} required autoComplete="username" />
        </label>
        <label>
          Password
          <input value={password} onChange={(event) => setPassword(event.target.value)} required type="password" autoComplete={mode === 'login' ? 'current-password' : 'new-password'} />
        </label>
        <button className="primary-button" type="submit" disabled={busy}>
          {busy ? 'Working…' : mode === 'login' ? 'Sign in securely' : 'Initialize control plane'}
        </button>
      </form>
    </section>
  );
}

function Dashboard({ data }: { data: JsonObject | null }) {
  const vault = asRecord(data?.vault);
  const files = asRecord(data?.files);
  const memory = asRecord(data?.memory);
  const jobs = asRecord(data?.jobs);
  const providers = Array.isArray(data?.providers) ? data.providers : [];
  return (
    <>
      <section className="hero-card">
        <div>
          <span className="status-label"><span className="status-dot" aria-hidden="true" /> Control plane ready</span>
          <h2>{stringValue(vault.name, 'Default Vault')}</h2>
          <p>{stringValue(vault.content_root, 'Vault storage is waiting for setup.')}</p>
        </div>
        <div className="hero-metric"><strong>{numberValue(data?.ready)}</strong><span>readiness flag</span></div>
      </section>
      <section className="metric-grid" aria-label="Vault metrics">
        <Metric label="Markdown notes" value={numberValue(files.notes)} detail={`${numberValue(files.entries)} tracked entries`} />
        <Metric label="Active memories" value={numberValue(memory.active)} detail={`${numberValue(memory.candidate)} candidates awaiting review`} />
        <Metric label="Pending jobs" value={numberValue(jobs.pending)} detail="Durable queue state" />
        <Metric label="Providers" value={providers.length} detail="Health is optional and degradable" />
      </section>
      <section className="panel-grid">
        <Panel title="Operator guidance" eyebrow="SAFE DEFAULTS">
          <ul className="check-list">
            <li>Use WebDAV credentials for Obsidian devices; never reuse the Admin password.</li>
            <li>Use MCP PATs with the smallest scope set needed by the Agent.</li>
            <li>Provider failures do not block Vault writes or lexical retrieval.</li>
          </ul>
        </Panel>
        <Panel title="Provider health" eyebrow="REDACTED">
          {providers.length === 0 ? <p className="muted">No provider health rows have been recorded.</p> : providers.map((provider, index) => {
            const record = asRecord(provider);
            return <div className="row-item" key={stringValue(record.provider_id, String(index))}><span>{stringValue(record.status, 'unknown')}</span><small>{stringValue(record.last_error, 'No recent error')}</small></div>;
          })}
        </Panel>
      </section>
    </>
  );
}

function ManagementPage({ page, data }: { page: Page; data: JsonObject | null }) {
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [oneTimeSecret, setOneTimeSecret] = useState<string | null>(null);
  useEffect(() => setOneTimeSecret(null), [page]);
  const titles: Record<Page, { eyebrow: string; description: string; action?: string }> = {
    dashboard: { eyebrow: 'OVERVIEW', description: 'Current service state.' },
    vault: { eyebrow: 'CANONICAL STORAGE', description: 'The Vault filesystem remains the source of truth.', action: 'Rescan Vault' },
    webdav: { eyebrow: 'DEVICE ACCESS', description: 'Dedicated, revocable credentials for Obsidian clients.', action: 'Create credential' },
    mcp: { eyebrow: 'AGENT ACCESS', description: 'Vault-bound PATs and OAuth resource-server metadata.', action: 'Create PAT' },
    providers: { eyebrow: 'ENRICHMENT', description: 'Provider configuration is encrypted and never returned as plaintext.', action: 'Add provider' },
    index: { eyebrow: 'DERIVED PROJECTION', description: 'Rebuildable FTS, links, topics, and knowledge-map status.', action: 'Rebuild index' },
    memory: { eyebrow: 'SOURCED CONTEXT', description: 'Inspect lifecycle, provenance, candidate review, and recall inputs.', action: 'Review candidates' },
    jobs: { eyebrow: 'DURABLE WORK', description: 'Leases, attempts, retries, and sanitized failure state.', action: 'Refresh' },
    audit: { eyebrow: 'SECURITY HISTORY', description: 'Append-oriented, redacted actions correlated by request ID.' },
    backup: { eyebrow: 'RECOVERY', description: 'Verified artifacts, staged restore, and maintenance recovery.', action: 'Check availability' },
    system: { eyebrow: 'RUNTIME', description: 'Listener, migration, and dependency diagnostics.' },
  };
  const descriptor = titles[page];
  async function runAction() {
    const actions: Partial<Record<Page, { method: string; path: string }>> = {
      vault: { method: 'POST', path: '/vault/rescan' },
      index: { method: 'POST', path: '/index/rebuild' },
      jobs: { method: 'GET', path: '/jobs?limit=50' },
      backup: { method: 'GET', path: '/backups' },
    };
    const action = actions[page];
    if (!action) {
      setActionMessage('This page is ready for its dedicated management form.');
      return;
    }
    setActionBusy(true);
    setActionMessage(null);
    try {
      const result = await adminApi.request<JsonObject>(action.path, { method: action.method });
      setActionMessage(action.method === 'GET' ? 'Latest state loaded.' : 'Operation admitted to the durable queue.');
      void result;
    } catch (requestError: unknown) {
      setActionMessage(requestError instanceof Error ? requestError.message : 'The operation failed.');
    } finally {
      setActionBusy(false);
    }
  }
  return (
    <section className="panel-grid panel-grid--wide">
      <Panel title={descriptor.description} eyebrow={descriptor.eyebrow}>
        {page === 'backup' ? <p className="notice">Restore is a global maintenance operation. It requires an explicit RESTORE confirmation and the current Admin password; if rollback checks cannot prove safety, use RECOVER after reviewing diagnostics.</p> : null}
        {page === 'memory' ? <p className="notice">Memory Markdown is canonical; SQLite rows, FTS, vectors, entities, relations, and candidates are rebuildable projections.</p> : null}
        {actionMessage ? <p className="notice" role="status">{actionMessage}</p> : null}
        {oneTimeSecret ? <SecretReveal secret={oneTimeSecret} onDismiss={() => setOneTimeSecret(null)} /> : null}
        {page === 'vault' ? <VaultForm data={data} onMessage={setActionMessage} /> : null}
        {page === 'backup' ? <BackupControls data={data} onMessage={setActionMessage} /> : null}
        {page === 'webdav' ? <WebDavForm onMessage={setActionMessage} onSecret={setOneTimeSecret} /> : null}
        {page === 'mcp' ? <McpForm onMessage={setActionMessage} onSecret={setOneTimeSecret} /> : null}
        {page === 'providers' ? <ProviderForm data={data} onMessage={setActionMessage} /> : null}
        {!['webdav', 'mcp', 'providers'].includes(page) && descriptor.action ? <button className="secondary-button action-button" type="button" disabled={actionBusy} onClick={() => void runAction()}>{actionBusy ? 'Working…' : descriptor.action}</button> : null}
        <DataInspector data={data} empty="No data has been returned yet." />
      </Panel>
    </section>
  );
}

function VaultForm({ data, onMessage }: { data: JsonObject | null; onMessage: (message: string) => void }) {
  const vault = asRecord(data?.vault ?? data);
  const [name, setName] = useState(stringValue(vault.name, 'Default Vault'));
  const [status, setStatus] = useState(stringValue(vault.status, 'active'));
  const [busy, setBusy] = useState(false);
  return <form className="compact-form" onSubmit={async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      await adminApi.request('/vault', { method: 'PATCH', body: {
        name,
        status,
        expected_settings_revision: typeof vault.settings_revision === 'number' ? vault.settings_revision : undefined,
      } });
      onMessage('Vault settings saved and the settings revision advanced.');
    } catch (requestError: unknown) {
      onMessage(requestError instanceof Error ? requestError.message : 'Vault update failed.');
    } finally { setBusy(false); }
  }}>
    <h3>Vault settings</h3>
    <div className="form-grid">
      <label>Name<input required value={name} onChange={(event) => setName(event.target.value)} /></label>
      <label>Status<select value={status} onChange={(event) => setStatus(event.target.value)}><option value="active">Active</option><option value="maintenance">Maintenance</option><option value="disabled">Disabled</option></select></label>
    </div>
    <small className="muted">Content root and reserved root are displayed by the server and are not editable from the browser.</small>
    <button className="primary-button" disabled={busy} type="submit">{busy ? 'Saving…' : 'Save Vault settings'}</button>
  </form>;
}

function WebDavForm({ onMessage, onSecret }: { onMessage: (message: string) => void; onSecret: (secret: string) => void }) {
  const [values, setValues] = useState({ name: '', username: '', password: '', permissions: 'read' });
  const [busy, setBusy] = useState(false);
  return <form className="compact-form" onSubmit={async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      const result = await adminApi.request<{ password: string }>('/webdav/credentials', { method: 'POST', body: { ...values, permissions: values.permissions.split(',').map((value) => value.trim()).filter(Boolean) } });
      onSecret(result.password);
      onMessage('WebDAV credential created. The password is shown only in the creation response.');
      setValues({ name: '', username: '', password: '', permissions: 'read' });
    } catch (requestError: unknown) {
      onMessage(requestError instanceof Error ? requestError.message : 'Credential creation failed.');
    } finally { setBusy(false); }
  }}>
    <h3>Create a device credential</h3>
    <div className="form-grid">
      <label>Name<input required value={values.name} onChange={(event) => setValues({ ...values, name: event.target.value })} placeholder="Obsidian laptop" /></label>
      <label>Username<input required value={values.username} onChange={(event) => setValues({ ...values, username: event.target.value })} /></label>
      <label>Password<input required type="password" value={values.password} onChange={(event) => setValues({ ...values, password: event.target.value })} autoComplete="new-password" /></label>
      <label>Permissions<input value={values.permissions} onChange={(event) => setValues({ ...values, permissions: event.target.value })} aria-describedby="webdav-permissions-help" /></label>
    </div>
    <small id="webdav-permissions-help" className="muted">Comma-separated: read, write, delete.</small>
    <button className="primary-button" disabled={busy} type="submit">{busy ? 'Creating…' : 'Create and show once'}</button>
  </form>;
}

function McpForm({ onMessage, onSecret }: { onMessage: (message: string) => void; onSecret: (secret: string) => void }) {
  const [values, setValues] = useState({ name: '', scopes: 'vault:discover,vault:read,memory:read' });
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
    setGrantValues((current) => ({
      ...current,
      issuer_id: current.issuer_id || stringValue(nextIssuers[0]?.id, ''),
    }));
  }
  useEffect(() => {
    let cancelled = false;
    Promise.all([
      adminApi.request<{ issuers: unknown[] }>('/mcp/oauth'),
      adminApi.request<{ grants: unknown[] }>('/mcp/oauth/grants'),
    ]).then(([issuerData, grantData]) => {
      if (cancelled) return;
      const nextIssuers = issuerData.issuers.map(asRecord);
      setIssuers(nextIssuers);
      setGrants(grantData.grants.map(asRecord));
      setGrantValues((current) => ({ ...current, issuer_id: current.issuer_id || stringValue(nextIssuers[0]?.id, '') }));
    }).catch((requestError: unknown) => {
      if (!cancelled) onMessage(requestError instanceof Error ? requestError.message : 'OAuth configuration could not be loaded.');
    });
    return () => { cancelled = true; };
  }, [onMessage]);
  return <>
    <form className="compact-form" onSubmit={async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      const result = await adminApi.request<{ secret: string }>('/mcp/tokens', { method: 'POST', body: { name: values.name, scopes: values.scopes.split(',').map((value) => value.trim()).filter(Boolean) } });
      onSecret(result.secret);
      onMessage('MCP PAT created. Copy the secret now; it will not be returned again.');
      setValues({ name: '', scopes: 'vault:discover,vault:read,memory:read' });
    } catch (requestError: unknown) {
      onMessage(requestError instanceof Error ? requestError.message : 'PAT creation failed.');
    } finally { setBusy(false); }
  }}>
    <h3>Create a Vault-bound PAT</h3>
    <div className="form-grid">
      <label>Name<input required value={values.name} onChange={(event) => setValues({ ...values, name: event.target.value })} placeholder="Personal agent" /></label>
      <label>Scopes<input required value={values.scopes} onChange={(event) => setValues({ ...values, scopes: event.target.value })} /></label>
    </div>
    <small className="muted">Delete, history, and memory:manage scopes are intentionally not included by default.</small>
    <button className="primary-button" disabled={busy} type="submit">{busy ? 'Creating…' : 'Create PAT and show once'}</button>
    </form>
    <form className="compact-form" onSubmit={async (event) => {
      event.preventDefault();
      setBusy(true);
      try {
        await adminApi.request('/mcp/oauth', {
          method: 'PUT',
          body: {
            ...issuerValues,
            discovery_url: null,
            enabled: true,
          },
        });
        await refreshOAuth();
        onMessage('OAuth resource-server issuer saved with normalized public RSA keys.');
      } catch (requestError: unknown) {
        onMessage(requestError instanceof Error ? requestError.message : 'OAuth issuer update failed.');
      } finally { setBusy(false); }
    }}>
      <h3>OAuth resource-server issuer</h3>
      <div className="form-grid">
        <label>Name<input required value={issuerValues.name} onChange={(event) => setIssuerValues({ ...issuerValues, name: event.target.value })} /></label>
        <label>Issuer URL<input required type="url" value={issuerValues.issuer_url} onChange={(event) => setIssuerValues({ ...issuerValues, issuer_url: event.target.value })} /></label>
        <label>Audience<input required value={issuerValues.audience} onChange={(event) => setIssuerValues({ ...issuerValues, audience: event.target.value })} /></label>
        <label>Protected resource URL<input required type="url" value={issuerValues.resource} onChange={(event) => setIssuerValues({ ...issuerValues, resource: event.target.value })} /></label>
      </div>
      <label>Public RSA JWKS<textarea required rows={5} value={issuerValues.jwks_cache_json} onChange={(event) => setIssuerValues({ ...issuerValues, jwks_cache_json: event.target.value })} placeholder='{"keys":[{"kty":"RSA","kid":"…","alg":"RS256","n":"…","e":"AQAB"}]}' /></label>
      <small className="muted">Symmetric keys and private key fields are rejected. Update this public cache when the issuer rotates keys.</small>
      <button className="secondary-button" disabled={busy} type="submit">{busy ? 'Saving…' : 'Save OAuth issuer'}</button>
    </form>
    <form className="compact-form" onSubmit={async (event) => {
      event.preventDefault();
      setBusy(true);
      try {
        await adminApi.request('/mcp/oauth/grants', {
          method: 'POST',
          body: {
            issuer_id: grantValues.issuer_id,
            subject: grantValues.subject,
            scopes: grantValues.scopes.split(',').map((value) => value.trim()).filter(Boolean),
          },
        });
        await refreshOAuth();
        onMessage('OAuth subject grant saved for this Vault.');
        setGrantValues((current) => ({ ...current, subject: '' }));
      } catch (requestError: unknown) {
        onMessage(requestError instanceof Error ? requestError.message : 'OAuth grant update failed.');
      } finally { setBusy(false); }
    }}>
      <h3>Vault-bound OAuth subject grants</h3>
      <div className="form-grid">
        <label>Issuer<select required value={grantValues.issuer_id} onChange={(event) => setGrantValues({ ...grantValues, issuer_id: event.target.value })}><option value="">Select an issuer</option>{issuers.map((issuer) => <option key={stringValue(issuer.id, '')} value={stringValue(issuer.id, '')}>{stringValue(issuer.name, stringValue(issuer.issuer_url, 'Issuer'))}</option>)}</select></label>
        <label>Subject<input required value={grantValues.subject} onChange={(event) => setGrantValues({ ...grantValues, subject: event.target.value })} /></label>
        <label>Scopes<input required value={grantValues.scopes} onChange={(event) => setGrantValues({ ...grantValues, scopes: event.target.value })} /></label>
      </div>
      <button className="secondary-button" disabled={busy || issuers.length === 0} type="submit">{busy ? 'Saving…' : 'Save subject grant'}</button>
      {grants.length === 0 ? <p className="muted">No active OAuth subject grants.</p> : grants.map((grant) => <div className="row-item" key={stringValue(grant.id, '')}><span>{stringValue(grant.subject, 'Unknown subject')}</span><small>{Array.isArray(grant.scopes) ? grant.scopes.join(', ') : ''}</small><button className="danger-button" type="button" disabled={busy} onClick={async () => {
        setBusy(true);
        try {
          await adminApi.request(`/mcp/oauth/grants/${stringValue(grant.id, '')}`, { method: 'DELETE' });
          await refreshOAuth();
          onMessage('OAuth subject grant revoked.');
        } catch (requestError: unknown) {
          onMessage(requestError instanceof Error ? requestError.message : 'OAuth grant revocation failed.');
        } finally { setBusy(false); }
      }}>Revoke</button></div>)}
    </form>
  </>;
}

function SecretReveal({ secret, onDismiss }: { secret: string; onDismiss: () => void }) {
  return <section className="secret-reveal" aria-live="assertive">
    <div><p className="eyebrow">SHOW ONCE</p><h3>Copy this secret now</h3><code>{secret}</code></div>
    <button className="secondary-button" type="button" onClick={onDismiss}>Hide secret</button>
  </section>;
}

function ProviderForm({ data, onMessage }: { data: JsonObject | null; onMessage: (message: string) => void }) {
  const providerMode = asRecord(data?.provider_mode);
  const [values, setValues] = useState({ name: '', provider_type: 'openai_compatible', base_url: '', secret: '' });
  const [mode, setMode] = useState(stringValue(providerMode.mode, 'disabled'));
  const [modeRevision, setModeRevision] = useState<number | null>(typeof providerMode.revision === 'number' ? providerMode.revision : null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setMode(stringValue(providerMode.mode, 'disabled'));
    setModeRevision(typeof providerMode.revision === 'number' ? providerMode.revision : null);
  }, [providerMode.mode, providerMode.revision]);
  async function saveMode() {
    setBusy(true);
    try {
      const result = await adminApi.request<{ revision: number }>('/providers/mode', {
        method: 'PUT',
        body: {
          mode,
          expected_revision: modeRevision ?? undefined,
        },
      });
      setModeRevision(result.revision);
      onMessage('Vault provider privacy mode saved. Provider calls now follow this explicit policy.');
    } catch (requestError: unknown) {
      onMessage(requestError instanceof Error ? requestError.message : 'Provider privacy mode update failed.');
    } finally { setBusy(false); }
  }
  return <>
    <section className="compact-form">
      <h3>Vault provider privacy mode</h3>
      <div className="form-grid">
        <label>Mode<select value={mode} onChange={(event) => setMode(event.target.value)}><option value="disabled">Disabled</option><option value="local_only">Local endpoints only</option><option value="remote_allowed">Remote HTTPS allowed</option></select></label>
      </div>
      <small className="muted">Disabled is the safe default. Remote mode still applies endpoint, path-privacy, timeout, and concurrency policy.</small>
      <button className="secondary-button" disabled={busy} type="button" onClick={() => void saveMode()}>{busy ? 'Saving…' : 'Save privacy mode'}</button>
    </section>
    <form className="compact-form" onSubmit={async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      await adminApi.request('/providers', { method: 'POST', body: { ...values, enabled: true, secret: values.secret || null, settings: {} } });
      onMessage('Provider saved. The stored secret is represented only by a masked hint.');
      setValues({ name: '', provider_type: 'openai_compatible', base_url: '', secret: '' });
    } catch (requestError: unknown) {
      onMessage(requestError instanceof Error ? requestError.message : 'Provider creation failed.');
    } finally { setBusy(false); }
  }}>
    <h3>Add a provider</h3>
    <div className="form-grid">
      <label>Name<input required value={values.name} onChange={(event) => setValues({ ...values, name: event.target.value })} /></label>
      <label>Adapter<select value={values.provider_type} onChange={(event) => setValues({ ...values, provider_type: event.target.value })}><option value="openai_compatible">OpenAI-compatible</option><option value="openai_responses">OpenAI Responses</option><option value="anthropic_messages">Anthropic Messages</option><option value="embedding_http">Embedding HTTP</option></select></label>
      <label>Base URL<input required type="url" value={values.base_url} onChange={(event) => setValues({ ...values, base_url: event.target.value })} placeholder="https://provider.example/v1/" /></label>
      <label>API secret<input type="password" value={values.secret} onChange={(event) => setValues({ ...values, secret: event.target.value })} autoComplete="new-password" /></label>
    </div>
    <button className="primary-button" disabled={busy} type="submit">{busy ? 'Saving…' : 'Save provider securely'}</button>
    </form>
  </>;
}

function BackupControls({ data, onMessage }: { data: JsonObject | null; onMessage: (message: string) => void }) {
  const backups = Array.isArray(data?.backups) ? data.backups.map(asRecord) : [];
  const [selected, setSelected] = useState(stringValue(backups[0]?.id, ''));
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  async function run(path: string, options: { method: string; body?: unknown }, message: string) {
    setBusy(true);
    try {
      await adminApi.request<JsonObject>(path, options);
      onMessage(message);
    } catch (requestError: unknown) {
      onMessage(requestError instanceof Error ? requestError.message : 'Backup operation failed.');
    } finally { setBusy(false); }
  }
  return <section className="compact-form">
    <h3>Verified recovery artifacts</h3>
    <button className="primary-button" type="button" disabled={busy} onClick={() => void run('/backups', { method: 'POST' }, 'Backup admitted to the durable queue.')}>Create verified backup</button>
    <div className="form-grid">
      <label>Backup ID<select value={selected} onChange={(event) => setSelected(event.target.value)}><option value="">Select a completed backup</option>{backups.map((backup) => <option key={stringValue(backup.id, '')} value={stringValue(backup.id, '')}>{stringValue(backup.id, 'unknown')} · {stringValue(backup.status, 'unknown')}</option>)}</select></label>
      <label>Current Admin password<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" /></label>
    </div>
    <div className="button-row">
      <button className="secondary-button" type="button" disabled={busy || !selected} onClick={() => void run(`/backups/${selected}/verify`, { method: 'POST' }, 'Backup verification admitted to the durable queue.')}>Verify checksums</button>
      <button className="secondary-button" type="button" disabled={busy || !selected} onClick={() => void run('/restore/validate', { method: 'POST', body: { backup_id: selected } }, 'Restore archive validated without changing configured roots.')}>Validate restore</button>
    </div>
    <button className="danger-button" type="button" disabled={busy || !selected || password.length === 0} onClick={() => void run('/restore', { method: 'POST', body: { backup_id: selected, confirmation: 'RESTORE', password } }, 'Restore admitted; the data plane will enter maintenance mode.')}>Apply RESTORE operation</button>
    <button className="secondary-button" type="button" disabled={busy || password.length === 0} onClick={() => void run('/maintenance/recover', { method: 'POST', body: { confirmation: 'RECOVER', password } }, 'Maintenance recovery checks passed; the service is reopening.')}>RECOVER maintenance</button>
    <small className="muted">The master key is never included in ordinary backup artifacts. Keep the separate key export safe if encrypted provider secrets must be recovered.</small>
  </section>;
}

function Panel({ title, eyebrow, action, children }: { title: string; eyebrow: string; action?: string; children: ReactNode }) {
  return (
    <article className="panel">
      <div className="panel-heading">
        <div><p className="eyebrow">{eyebrow}</p><h2>{title}</h2></div>
        {action ? <button className="secondary-button" type="button">{action}</button> : null}
      </div>
      {children}
    </article>
  );
}

function Metric({ label, value, detail }: { label: string; value: number; detail: string }) {
  return <article className="metric-card"><span>{label}</span><strong>{value}</strong><small>{detail}</small></article>;
}

function DataInspector({ data, empty }: { data: JsonObject | null; empty: string }) {
  if (!data) return <p className="muted">{empty}</p>;
  return <pre className="data-inspector">{JSON.stringify(data, null, 2)}</pre>;
}

function InlineAlert({ message, onDismiss }: { message: string; onDismiss?: () => void }) {
  return <div className="inline-alert" role="alert"><span>{message}</span>{onDismiss ? <button type="button" onClick={onDismiss} aria-label="Dismiss error">Dismiss</button> : null}</div>;
}

function asRecord(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as JsonObject : {};
}

function numberValue(value: unknown): number {
  return typeof value === 'number' ? value : 0;
}

function stringValue(value: unknown, fallback: string): string {
  return typeof value === 'string' && value.length > 0 ? value : fallback;
}
