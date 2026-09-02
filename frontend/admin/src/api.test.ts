import { beforeEach, describe, expect, it, vi } from 'vitest';

import { AdminApiClient } from './api';

describe('Admin API client', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    document.cookie = 'mcp_vault_csrf=; Path=/; Max-Age=0';
  });

  it('keeps the CSRF token in memory and sends it only on mutations', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { csrf_token: 'csrf-1' }, request_id: '1' }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { changed: true }, request_id: '2' }), { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);
    const client = new AdminApiClient();

    await client.login('owner', 'password');
    await client.request('/session/password', { method: 'POST', body: { current_password: 'old', new_password: 'new' } });

    const mutation = fetchMock.mock.calls[1]?.[1] as RequestInit;
    expect(new Headers(mutation.headers).get('X-CSRF-Token')).toBe('csrf-1');
    expect(mutation.credentials).toBe('include');
    expect(JSON.stringify(mutation.body)).toContain('new');
  });

  it('reads first-Admin setup availability without a mutation token', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      new Response(
        JSON.stringify({ data: { setup_available: false }, request_id: 'setup-status' }),
        { status: 200 },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);
    const client = new AdminApiClient();

    await expect(client.setupStatus()).resolves.toEqual({ setup_available: false });

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/v1/setup',
      expect.objectContaining({ method: 'GET', credentials: 'include' }),
    );
    const options = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(new Headers(options.headers).has('X-CSRF-Token')).toBe(false);
  });

  it('submits first-Admin setup with only username and password', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      new Response(JSON.stringify({ data: { created: true }, request_id: 'setup' }), {
        status: 201,
      }),
    );
    vi.stubGlobal('fetch', fetchMock);
    const client = new AdminApiClient();

    await client.setup('owner', 'correct horse battery staple');

    const options = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(JSON.parse(String(options.body))).toEqual({
      username: 'owner',
      password: 'correct horse battery staple',
    });
    expect(String(options.body)).not.toContain('bootstrap');
  });

  it('restores a cookie-backed session and reuses its CSRF token for mutations', async () => {
    document.cookie = 'mcp_vault_csrf=csrf-restored; Path=/';
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            data: {
              user_id: 'admin-1',
              username: 'owner',
              expires_at: null,
              csrf_token: null,
            },
            request_id: 'restore',
          }),
          { status: 200 },
        ),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ data: { changed: true }, request_id: 'mutation' }), {
          status: 200,
        }),
      );
    vi.stubGlobal('fetch', fetchMock);
    const client = new AdminApiClient();

    await expect(client.restoreSession()).resolves.toMatchObject({ username: 'owner' });
    await client.request('/vault', { method: 'PATCH', body: { name: 'Renamed' } });

    const restore = fetchMock.mock.calls[0]?.[1] as RequestInit;
    expect(restore.method).toBe('GET');
    const mutation = fetchMock.mock.calls[1]?.[1] as RequestInit;
    expect(new Headers(mutation.headers).get('X-CSRF-Token')).toBe('csrf-restored');
  });

  it('clears a stale CSRF cookie when the server rejects session restoration', async () => {
    document.cookie = 'mcp_vault_csrf=csrf-stale; Path=/';
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            error: { code: 'admin_session_expired', message: 'expired' },
            request_id: 'expired',
          }),
          { status: 401 },
        ),
      ),
    );
    const client = new AdminApiClient();

    await expect(client.restoreSession()).resolves.toBeNull();
    expect(document.cookie).not.toContain('mcp_vault_csrf=');
  });

  it('scopes Vault-owned requests while keeping global Provider routes unchanged', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => (
      new Response(JSON.stringify({ data: { ok: true }, request_id: 'scope' }), { status: 200 })
    ));
    vi.stubGlobal('fetch', fetchMock);
    const client = new AdminApiClient();
    client.setVaultSlug('work');

    await client.request('/jobs/overview?limit=50');
    await client.request('/providers/provider-1');

    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/vaults/work/jobs/overview?limit=50');
    expect(fetchMock.mock.calls[1]?.[0]).toBe('/api/v1/providers/provider-1');
  });
});
