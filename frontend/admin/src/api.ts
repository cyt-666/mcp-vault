export type ApiEnvelope<T> = {
  data?: T;
  error?: {
    code: string;
    message: string;
    fields?: Record<string, string>;
  };
  request_id: string;
};

export class AdminApiError extends Error {
  readonly code: string;
  readonly fields: Record<string, string>;
  readonly status: number;

  constructor(status: number, code: string, message: string, fields: Record<string, string> = {}) {
    super(message);
    this.name = 'AdminApiError';
    this.status = status;
    this.code = code;
    this.fields = fields;
  }
}

export type RequestOptions = Omit<RequestInit, 'body'> & {
  body?: unknown;
};

export type AdminSession = {
  user_id: string;
  username: string;
  expires_at: number | null;
  csrf_token: string | null;
};

const CSRF_COOKIE_NAME = 'mcp_vault_csrf';

function readCsrfCookie(): string | null {
  if (typeof document === 'undefined') return null;
  for (const part of document.cookie.split(';')) {
    const [name, ...value] = part.trim().split('=');
    if (name === CSRF_COOKIE_NAME && value.length > 0) {
      return decodeURIComponent(value.join('='));
    }
  }
  return null;
}

function clearCsrfCookie() {
  if (typeof document === 'undefined') return;
  document.cookie = `${CSRF_COOKIE_NAME}=; Path=/; Max-Age=0; SameSite=Strict`;
}

export class AdminApiClient {
  private csrfToken: string | null = readCsrfCookie();

  async request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const method = (options.method ?? 'GET').toUpperCase();
    const headers = new Headers(options.headers);
    headers.set('Accept', 'application/json');
    if (options.body !== undefined) {
      headers.set('Content-Type', 'application/json');
    }
    if (!['GET', 'HEAD', 'OPTIONS'].includes(method) && this.csrfToken) {
      headers.set('X-CSRF-Token', this.csrfToken);
    }
    const response = await fetch(`/api/v1${path}`, {
      ...options,
      method,
      headers,
      credentials: 'include',
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
    });
    const payload = (await response.json()) as ApiEnvelope<T>;
    if (!response.ok || !payload.data) {
      const error = payload.error;
      throw new AdminApiError(
        response.status,
        error?.code ?? 'request_failed',
        error?.message ?? '管理端请求失败。',
        error?.fields,
      );
    }
    return payload.data;
  }

  async login(username: string, password: string) {
    const response = await this.request<AdminSession>('/session', {
      method: 'POST',
      body: { username, password },
    });
    this.csrfToken = response.csrf_token;
    return response;
  }

  async restoreSession(): Promise<AdminSession | null> {
    const csrfToken = readCsrfCookie();
    if (!csrfToken) {
      this.csrfToken = null;
      return null;
    }
    this.csrfToken = csrfToken;
    try {
      return await this.request<AdminSession>('/session');
    } catch (error: unknown) {
      if (error instanceof AdminApiError && error.status === 401) {
        this.clearSession();
        return null;
      }
      throw error;
    }
  }

  async setupStatus() {
    return this.request<{ setup_available: boolean }>('/setup');
  }

  async setup(username: string, password: string) {
    return this.request('/setup', {
      method: 'POST',
      body: { username, password },
    });
  }

  async logout() {
    try {
      await this.request('/session', { method: 'DELETE' });
    } finally {
      this.csrfToken = null;
    }
  }

  clearSession() {
    this.csrfToken = null;
    clearCsrfCookie();
  }
}

export const adminApi = new AdminApiClient();
