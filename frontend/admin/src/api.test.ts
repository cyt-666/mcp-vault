import { beforeEach, describe, expect, it, vi } from 'vitest';

import { AdminApiClient } from './api';

describe('Admin API client', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
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
});
