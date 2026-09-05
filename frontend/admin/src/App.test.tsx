import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App, isInsecureLanAdminLocation } from './App';
import { AdminApiError, adminApi } from './api';
import { Dashboard, ManagementPage } from './pages';
import { formatRequestError, jobErrorLabel } from './view-model';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

function setInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('Admin 管理界面', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    window.history.replaceState(null, '', '/');
    adminApi.setVaultSlug(null);
    vi.spyOn(adminApi, 'restoreSession').mockResolvedValue(null);
    vi.spyOn(adminApi, 'setupStatus').mockResolvedValue({ setup_available: false });
  });

  it('仅在非回环 HTTP 管理地址显示明文传输风险', () => {
    expect(isInsecureLanAdminLocation({ protocol: 'http:', hostname: '192.168.1.20' })).toBe(true);
    expect(isInsecureLanAdminLocation({ protocol: 'http:', hostname: 'localhost' })).toBe(false);
    expect(isInsecureLanAdminLocation({ protocol: 'https:', hostname: 'mcp-vault.cyt.cool' })).toBe(false);
  });

  it('刷新后恢复有效管理会话而不显示登录页', async () => {
    vi.mocked(adminApi.restoreSession).mockResolvedValue({
      user_id: 'admin-1',
      username: 'owner',
      expires_at: null,
      csrf_token: null,
    });
    vi.spyOn(adminApi, 'request').mockResolvedValue({ ready: true });
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () => root.render(<App />));

    expect(container.textContent).toContain('管理控制台');
    expect(container.textContent).toContain('总览');
    expect(container.textContent).not.toContain('欢迎回来');
    expect(container.querySelector<HTMLImageElement>('img.sidebar-logo')?.getAttribute('src'))
      .toBe('/mcp-vault-logo.png');
    expect(adminApi.setupStatus).not.toHaveBeenCalled();

    await act(async () => root.unmount());
  });

  it('记忆页面从任务总览读取较早创建但仍在运行的任务', async () => {
    vi.mocked(adminApi.restoreSession).mockResolvedValue({
      user_id: 'admin-1',
      username: 'owner',
      expires_at: null,
      csrf_token: null,
    });
    const request = vi.spyOn(adminApi, 'request').mockImplementation(async (path) => {
      if (path === '/vaults') {
        return {
          vaults: [{
            id: 'vault-1',
            slug: 'default',
            name: '默认 Vault',
            status: 'active',
            availability: 'ready',
            content_root: '/srv/default',
          }],
        };
      }
      if (path === '/vaults/default/dashboard') return { ready: true };
      if (path === '/vaults/default/memories?limit=50') return { memories: [] };
      if (path === '/vaults/default/memory/extraction') {
        return {
          policy: { enabled: true, source_mode: 'automatic', max_evidence_per_note: 3 },
          readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
        };
      }
      if (path === '/vaults/default/jobs/overview?limit=50') {
        return {
          running: [{
            id: 'older-running-memory-job',
            job_type: 'memory.extract',
            status: 'running',
            progress: { completed: 1, total: 2, current_index: 2 },
          }],
          queued: [],
          retry_wait: [],
          history: [],
        };
      }
      return {};
    });
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () => root.render(<App />));
    const memoryNav = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('记忆'),
    ) as HTMLButtonElement;
    await act(async () => {
      memoryNav.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(request).toHaveBeenCalledWith('/vaults/default/jobs/overview?limit=50');
    expect(container.textContent).toContain('任务执行中 · 50%');
    expect(container.textContent).toContain('older-ru…ry-job');

    await act(async () => root.unmount());
  });

  it('切换 Vault 后所有页面请求使用新的显式作用域', async () => {
    vi.mocked(adminApi.restoreSession).mockResolvedValue({
      user_id: 'admin-1',
      username: 'owner',
      expires_at: null,
      csrf_token: null,
    });
    const request = vi.spyOn(adminApi, 'request').mockImplementation(async (path) => {
      if (path === '/vaults') {
        return {
          vaults: [
            { id: 'v1', slug: 'default', name: '默认', status: 'active', availability: 'ready', content_root: '/default' },
            { id: 'v2', slug: 'work', name: '工作', status: 'active', availability: 'ready', content_root: '/work' },
          ],
        };
      }
      if (path.endsWith('/dashboard')) return { ready: true, vault: { name: path.includes('/work/') ? '工作' : '默认' } };
      return {};
    });
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () => root.render(<App />));
    const selector = container.querySelector<HTMLSelectElement>('.vault-switcher select');
    expect(selector?.value).toBe('default');
    await act(async () => {
      if (!selector) throw new Error('Vault selector missing');
      selector.value = 'work';
      selector.dispatchEvent(new Event('change', { bubbles: true }));
    });

    expect(request).toHaveBeenCalledWith('/vaults/work/dashboard');
    expect(new URLSearchParams(window.location.search).get('vault')).toBe('work');

    await act(async () => root.unmount());
  });

  it('已初始化时只显示中文管理员登录', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () => root.render(<App />));

    expect(container.textContent).toContain('你的知识库，始终由你掌控');
    expect(container.textContent).toContain('管理员登录');
    expect(container.textContent).toContain('WebDAV 密码和 MCP Token 是彼此独立的凭据');
    expect(container.textContent).not.toContain('首次初始化');
    expect(container.textContent).not.toContain('Control plane');
    expect(container.querySelector<HTMLImageElement>('img.brand-mark')?.getAttribute('src'))
      .toBe('/mcp-vault-logo.png');

    await act(async () => root.unmount());
  });

  it('未初始化时只要求首个管理员账号和密码', async () => {
    vi.mocked(adminApi.setupStatus).mockResolvedValue({ setup_available: true });
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () => root.render(<App />));

    expect(container.textContent).toContain('首次初始化');
    expect(container.textContent).toContain('创建第一个管理员');
    expect(container.textContent).toContain('设置管理员账号和密码即可完成初始化');
    expect(container.textContent).toContain('纯英文至少 12 个字符');
    expect(container.textContent).toContain('常用汉字至少 4 个');
    expect(container.textContent).toContain('无需强制包含大小写、数字或符号');
    expect(container.textContent).not.toContain('引导 Token');
    expect(container.textContent).not.toContain('bootstrap-token');
    expect(container.textContent).not.toContain('欢迎回来');

    await act(async () => root.unmount());
  });

  it('WebDAV 密码表单也直接解释相同规则', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () =>
      root.render(
        <ManagementPage
          page="webdav"
          data={{ credentials: [], webdav_endpoint: 'http://127.0.0.1:8080/dav/v1/vaults/default/' }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('WebDAV 密码');
    expect(container.textContent).toContain('纯英文至少 12 个字符');
    expect(container.textContent).toContain('password123');

    await act(async () => root.unmount());
  });

  it('MCP 页面默认提供内置 ChatGPT OAuth，并把外部 IdP 放在高级区', async () => {
    vi.spyOn(adminApi, 'request').mockImplementation(async (path) => {
      if (path === '/mcp/oauth') return { issuers: [] };
      if (path === '/mcp/oauth/grants') return { grants: [] };
      return {};
    });
    const mcpEndpoint = 'https://vault.example.test/mcp/v1/vaults/default';
    const metadataUrl = 'https://vault.example.test/.well-known/oauth-protected-resource/mcp/v1/vaults/default';
    const authorizationMetadataUrl = 'https://vault.example.test/.well-known/oauth-authorization-server';
    const container = document.createElement('div');
    const root = createRoot(container);

    await act(async () =>
      root.render(
        <ManagementPage
          page="mcp"
          data={{
            tokens: [],
            mcp_endpoint: mcpEndpoint,
            oauth_protected_resource_metadata_url: metadataUrl,
            oauth_authorization_server_metadata_url: authorizationMetadataUrl,
            local_oauth: { configured: false, user: null },
            supported_mcp_revisions: ['2026-07-28'],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('OAuth 资源元数据');
    expect(container.textContent).toContain('内置 OAuth 服务元数据');
    expect(container.textContent).toContain('直接使用 MCP Vault 登录，不需要部署外部 OAuth 服务');
    expect(container.textContent).toContain('启用内置 OAuth');
    expect(container.textContent).toContain('不是 Admin 密码');
    expect(container.textContent).toContain('高级：外部 OAuth/OIDC 兼容');
    const advanced = container.querySelector<HTMLDetailsElement>('details.advanced-section');
    await act(async () => {
      if (advanced) advanced.open = true;
      advanced?.dispatchEvent(new Event('toggle', { bubbles: true }));
    });

    expect(container.textContent).toContain('外部授权服务器执行授权码 + PKCE（S256）');
    expect(container.textContent).toContain('ChatGPT 发现地址');
    const resourceInput = container.querySelector<HTMLInputElement>('input[readonly][type="url"]');
    expect(resourceInput?.value).toBe(mcpEndpoint);
    const audienceInput = Array.from(container.querySelectorAll<HTMLInputElement>('input'))
      .find((input) => input.parentElement?.textContent?.includes('Audience'));
    expect(audienceInput?.value).toBe(mcpEndpoint);

    await act(async () => root.unmount());
  });

  it('密码策略错误会返回可执行的具体说明', () => {
    const message = formatRequestError(
      new AdminApiError(422, 'password_policy', 'password does not satisfy policy'),
    );

    expect(message).toContain('纯英文至少 12 个字符');
    expect(message).toContain('常用汉字至少 4 个');
    expect(message).toContain('无需强制组合大小写、数字或符号');
    expect(message).not.toContain('请使用更长且不常见的密码');
  });

  it('任务页面优先展示中文摘要并默认折叠原始 JSON', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="jobs"
          data={{
            running: [
              {
                id: '019d-memory-job',
                job_type: 'memory.extract',
                status: 'running',
                attempts: 3,
                max_attempts: 10,
                progress: {
                  phase: 'extracting_note',
                  completed: 0,
                  total: 228,
                  current_index: 1,
                  current_path: 'projects/current.md',
                  items_published: 0,
                  empty_sets_published: 0,
                  source_ingestion_failures: 0,
                  generated_output_failures: 0,
                },
                last_error: 'provider_response_read_failed',
              },
            ],
            queued: [
              {
                id: '019d-test-job',
                job_type: 'vault.reconcile',
                status: 'queued',
                attempts: 0,
                max_attempts: 3,
                progress: 0.25,
              },
              {
                id: '019d-embedding-job',
                job_type: 'embedding.rebuild',
                status: 'queued',
                attempts: 0,
                max_attempts: 10,
                details: {
                  projection_version: 3,
                  model_id: '01a066c8-7710-72d1-9699-5f37819fae27',
                  source_type: 'note',
                  source_count: 37,
                },
              },
            ],
            retry_wait: [],
            history: [
              {
                id: '019d-source-reconcile-job',
                job_type: 'memory.source_reconcile',
                status: 'completed',
                attempts: 1,
                max_attempts: 10,
                progress: {
                  phase: 'completed',
                  sources_checked: 1,
                  current: 0,
                  moved: 1,
                  changed: 0,
                  deleted: 0,
                  memories_hidden: 0,
                },
              },
              {
                id: '019d-completed-job',
                job_type: 'index.rebuild',
                status: 'completed',
                attempts: 1,
                max_attempts: 5,
                progress: null,
              },
              {
                id: '019d-partial-memory-job',
                job_type: 'memory.extract',
                status: 'completed',
                attempts: 1,
                max_attempts: 5,
                progress: {
                  phase: 'completed_with_errors',
                  completed: 178,
                  total: 178,
                  notes_evaluated: 178,
                  source_ingestion_failures: 1,
                  source_ingestion_failure_notes: [{
                    index: 9,
                    path: 'notes/binary.md',
                    error_code: 'memory_source_not_utf8',
                  }],
                  generated_output_failures: 1,
                  generated_output_failure_notes: [{
                    index: 10,
                    path: 'notes/bad.md',
                    error_code: 'provider_schema_invalid',
                    schema_issue: 'required_property_missing',
                    schema_path: '$.memories[0].content',
                  }],
                },
              },
              {
                id: '019d-circuit-memory-job',
                job_type: 'memory.extract',
                status: 'failed',
                attempts: 1,
                max_attempts: 5,
                last_error: 'memory_extract_output_failure_limit',
                progress: {
                  phase: 'stopped_output_failures',
                  completed: 3,
                  total: 178,
                  generated_output_failures: 3,
                },
              },
            ],
            counts: {
              running: 1,
              queued: 1595,
              retry_wait: 0,
              completed: 3,
              failed: 1,
              cancelled: 0,
              active: 1596,
              terminal: 4,
            },
            truncated: { running: false, queued: true, retry_wait: false },
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('Vault 重新扫描');
    expect(container.textContent).toContain('重建知识索引');
    expect(container.textContent).toContain('重建语义向量');
    expect(container.textContent).toContain('生成 37 个笔记分块向量');
    expect(container.textContent).toContain('投影 v3');
    expect(container.textContent).toContain('正在执行（1）');
    expect(container.textContent).toContain('等待执行（1595）');
    expect(container.textContent).toContain('已结束历史（显示 4 / 共 4）');
    expect(container.textContent).toContain('等待中');
    expect(container.textContent).toContain('进度 25%');
    expect(container.textContent).toContain('进度 100%');
    expect(container.textContent).toContain('生成来源当前记忆集合');
    expect(container.textContent).toContain('协调当前记忆来源');
    expect(container.textContent).toContain('来源协调完成：检查 1 个当前集合');
    expect(container.textContent).toContain('正在处理第 1 / 228 篇：projects/current.md');
    expect(container.textContent).toContain('上次尝试：AI 服务已接受请求，但响应正文读取失败');
    expect(container.textContent).toContain('完成但有失败');
    expect(container.textContent).toContain('源文件无法处理 1 篇（模型未调用）');
    expect(container.textContent).toContain('最近源文件问题 notes/binary.md：笔记不是 UTF-8 文本');
    expect(container.textContent).toContain('模型输出校验失败 1 篇（模型已调用）');
    expect(container.textContent).toContain('最近模型输出问题 notes/bad.md');
    expect(container.textContent).toContain('缺少必填字段 $.memories[0].content');
    expect(container.textContent).toContain('已处理 3 / 178 篇，因连续模型输出错误暂停');
    expect(container.textContent).not.toContain('已完成进度 0%');
    expect(container.textContent).toContain('高级：查看原始响应');
    expect((container.querySelector('details.raw-details') as HTMLDetailsElement).open).toBe(false);

    await act(async () => root.unmount());
  });

  it('索引覆盖率使用后端分子分母而不是把详情对象显示成零', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <Dashboard
          data={{
            ready: true,
            vault: { name: 'default', status: 'active' },
            files: { notes: 178, attachments: 0, entries: 178 },
            index: { indexed_notes: 178, total_notes: 178, coverage_ratio: 1, coverage: { complete: true } },
            memory: { current: 0, explicit: 0, note_derived: 0 },
            jobs: { pending: 0 },
            providers: [],
          }}
          onNavigate={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('索引覆盖率');
    expect(container.textContent).toContain('100%');
    expect(container.textContent).toContain('178 / 178 篇已索引');

    await act(async () => root.unmount());
  });

  it('知识索引页面区分全文覆盖率和笔记语义向量覆盖率', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({
      source_chunks: 220,
      queued_chunks: 110,
      pruned_vectors: 2,
      jobs: 2,
    });
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="index"
          data={{
            status: {
              indexed_notes: 178,
              total_notes: 178,
              coverage_ratio: 1,
              indexed_entries: 180,
              indexed_bytes: 1024,
              analyzer_version: 1,
              revision: 2,
            },
            note_semantic: {
              configured: true,
              external_model_id: 'embedding-model',
              source_chunks: 220,
              indexed_chunks: 110,
              stale_vectors: 2,
              coverage_ratio: 0.5,
              blockers: ['embedding_coverage_incomplete'],
            },
          }}
          onRefresh={onRefresh}
        />,
      ),
    );

    expect(container.textContent).toContain('笔记语义召回');
    expect(container.textContent).toContain('50%');
    expect(container.textContent).toContain('110 / 220 个内容分块');
    expect(container.textContent).toContain('仍有笔记分块等待生成向量');
    expect(container.textContent).toContain('“重建知识索引”完成不代表向量任务完成');
    const rebuild = Array.from(container.querySelectorAll('button')).find(
      (button) => button.textContent === '生成缺失向量',
    ) as HTMLButtonElement;
    await act(async () => {
      rebuild.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(request).toHaveBeenCalledWith('/index/embeddings/rebuild', { method: 'POST' });
    expect(onRefresh).toHaveBeenCalledOnce();

    await act(async () => root.unmount());
  });

  it('AI 服务页面展示模型登记和自动记忆角色绑定', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="providers"
          data={{
            provider_mode: { mode: 'local_only', revision: 1 },
            providers: [{
              id: 'provider-1',
              name: '小米 MiMo',
              provider_type: 'xiaomi_mimo',
              base_url: 'https://api.xiaomimimo.com/v1/',
              secret: { configured: false },
              health: { status: 'healthy' },
            }],
            models: [
              {
                id: 'model-1',
                provider_id: 'provider-1',
                external_model_id: 'mimo-v2.5-pro',
                capabilities: { structured_output: true },
                settings: {},
              },
              {
                id: 'model-2',
                provider_id: 'provider-1',
                external_model_id: 'mimo-v2.5',
                capabilities: { structured_output: true, max_output_tokens: 8192 },
                settings: { openai_compatibility_preset: 'auto' },
              },
            ],
            bindings: [{ role: 'memory_extraction', model_id: 'model-1', vault_id: 'vault-1', revision: 1 }],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('mimo-v2.5-pro');
    expect(container.textContent).toContain('mimo-v2.5');
    expect(container.textContent).toContain('小米 MiMo · JSON Object');
    expect(container.textContent).toContain('手动登记模型');
    expect(container.textContent).toContain('提供商兼容预设');
    expect(container.textContent).toContain('跟随 AI 服务（推荐）');
    expect(container.textContent).toContain('模型声明的生成上限');
    expect(container.textContent).toContain('思考开启');
    expect(container.textContent).toContain('单次上限 32768');
    expect(container.textContent).toContain('DeepSeek');
    expect(container.textContent).toContain('智谱 GLM');
    expect(container.textContent).toContain('Kimi / Moonshot');
    expect(container.textContent).toContain('Google Gemini');
    expect(container.textContent).toContain('阿里千问 / DashScope');
    expect(container.textContent).toContain('模型用途');
    expect(container.textContent).toContain('自动生成长期记忆');
    expect(container.textContent).toContain('已绑定');

    const providerTypeSelect = Array.from(container.querySelectorAll('label'))
      .find((label) => label.textContent?.startsWith('AI 服务类型'))
      ?.querySelector('select') as HTMLSelectElement;
    await act(async () => {
      providerTypeSelect.value = 'google_gemini';
      providerTypeSelect.dispatchEvent(new Event('change', { bubbles: true }));
    });
    const baseUrlInput = Array.from(container.querySelectorAll('label'))
      .find((label) => label.textContent?.startsWith('Base URL'))
      ?.querySelector('input') as HTMLInputElement;
    expect(baseUrlInput.value).toBe('https://generativelanguage.googleapis.com/v1beta/openai/');

    await act(async () => root.unmount());
  });

  it('删除 AI 服务会说明影响并携带当前 revision 调用后端', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({
      deleted: true,
      provider_id: 'provider-1',
      models_deleted: 1,
      bindings_deleted: 1,
      embeddings_deleted: 4,
      secrets_deleted: 1,
    });
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="providers"
          data={{
            provider_mode: { mode: 'remote_allowed', revision: 1 },
            providers: [{
              id: 'provider-1',
              name: '待删除服务',
              provider_type: 'deepseek',
              base_url: 'https://api.deepseek.com/v1/',
              revision: 3,
              secret: { configured: true, hint: 'sk-…1234' },
              health: { status: 'healthy' },
            }],
            models: [{
              id: 'model-1',
              provider_id: 'provider-1',
              external_model_id: 'deepseek-chat',
              capabilities: { structured_output: true },
              settings: {},
            }],
            bindings: [{ role: 'memory_extraction', model_id: 'model-1', vault_id: 'vault-1', revision: 1 }],
          }}
          onRefresh={onRefresh}
        />,
      ),
    );

    const deleteButton = container.querySelector(
      'button[aria-label="删除 AI 服务 待删除服务"]',
    ) as HTMLButtonElement;
    expect(deleteButton).not.toBeNull();
    await act(async () => {
      deleteButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('将删除 1 个模型');
    expect(confirm.mock.calls[0][0]).toContain('当前可见 1 个');
    expect(confirm.mock.calls[0][0]).toContain('Vault 原始笔记、长期记忆、后台任务历史和审计记录不会被删除');
    expect(request).toHaveBeenCalledWith(
      '/providers/provider-1?expected_revision=3',
      { method: 'DELETE' },
    );
    expect(container.textContent).toContain('清理 1 个模型、1 个用途绑定和 4 条可重建向量记录');
    expect(onRefresh).toHaveBeenCalledOnce();

    await act(async () => root.unmount());
  });

  it('编辑 AI 服务会保留未替换密钥和未展示的安全设置', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({ revision: 5 });
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="providers"
          data={{
            provider_mode: { mode: 'remote_allowed', revision: 1 },
            providers: [{
              id: 'provider-edit',
              name: '待编辑服务',
              provider_type: 'deepseek',
              base_url: 'https://api.deepseek.com/v1/',
              enabled: true,
              revision: 4,
              settings: {
                timeout_ms: 30_000,
                connect_timeout_ms: 5_000,
                max_retries: 2,
                max_concurrency: 4,
                max_request_bytes: 2_097_152,
                max_response_bytes: 4_194_304,
                allow_private_networks: false,
                headers: { 'X-Project': 'project-a' },
                organization: null,
                model_cache_dir: null,
              },
              secret: { configured: true, hint: 'sk-…1234' },
              health: { status: 'healthy' },
            }],
            models: [],
            bindings: [],
          }}
          onRefresh={onRefresh}
        />,
      ),
    );

    const form = container.querySelector('form[aria-label="编辑 待编辑服务"]') as HTMLFormElement;
    const name = form.querySelector('input[name="provider-name"]') as HTMLInputElement;
    const timeout = form.querySelector('input[name="provider-timeout"]') as HTMLInputElement;
    const concurrency = form.querySelector('input[name="provider-concurrency"]') as HTMLInputElement;
    await act(async () => {
      setInputValue(name, '已编辑服务');
      setInputValue(timeout, '90');
      setInputValue(concurrency, '8');
    });
    await act(async () => {
      form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    });

    expect(request).toHaveBeenCalledWith('/providers/provider-edit', {
      method: 'PATCH',
      body: expect.objectContaining({
        name: '已编辑服务',
        provider_type: 'deepseek',
        base_url: 'https://api.deepseek.com/v1/',
        enabled: true,
        secret: null,
        expected_revision: 4,
        settings: expect.objectContaining({
          timeout_ms: 90_000,
          connect_timeout_ms: 5_000,
          max_retries: 2,
          max_concurrency: 8,
          max_request_bytes: 2_097_152,
          max_response_bytes: 4_194_304,
          headers: { 'X-Project': 'project-a' },
        }),
      }),
    });
    expect(container.textContent).toContain('原密钥保持不变');
    expect(onRefresh).toHaveBeenCalledOnce();

    await act(async () => root.unmount());
  });

  it('任务错误会区分输出截断、非 JSON 和空内容', () => {
    expect(jobErrorLabel('provider_output_truncated')).toContain('Token 上限');
    expect(jobErrorLabel('provider_structured_json_invalid')).toContain('不是完整的 JSON');
    expect(jobErrorLabel('provider_final_content_missing')).toContain('没有最终文本内容');
    expect(jobErrorLabel('embedding_dimension_mismatch')).toContain('默认维度');
    expect(jobErrorLabel('provider_schema_invalid')).toContain('返回了 JSON');
    expect(jobErrorLabel('memory_extract_output_failure_limit')).toContain('连续 3 次');
    expect(jobErrorLabel('memory_source_too_large')).toContain('512 KiB');
    expect(jobErrorLabel('memory_set_too_many_items')).toContain('每篇笔记上限');
    expect(jobErrorLabel('memory_set_item_invalid')).toContain('记忆内容');
    expect(jobErrorLabel('memory_set_snapshot_hash_mismatch')).toContain('来源内容不一致');
    expect(jobErrorLabel('memory_source_reconcile_progress_failed')).toContain('协调结果');
    expect(jobErrorLabel('memory_phase2_prepared_invalid')).toContain('旧版记忆任务已退役');
  });

  it('旧版检索回填数据不会重新暴露在当前记忆页面', async () => {
    const request = vi.spyOn(adminApi, 'request');
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            extraction: {
              policy: { enabled: true, source_mode: 'automatic' },
              readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
            },
            retrieval: {
              coverage: {
                prompt_version: 'memory-retrieval-v1',
                target_languages: ['source', 'zh-Hans', 'en'],
                eligible: 17,
                current: 9,
                pending: 7,
                failed: 1,
                estimated_batches: 1,
              },
              active_job: null,
            },
            memory_jobs: [],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('笔记当前记忆集合');
    expect(container.textContent).toContain('每篇模型调用1 次');
    expect(container.textContent).not.toContain('跨语言检索');
    expect(container.textContent).not.toContain('回填现有记忆');
    expect(request).not.toHaveBeenCalled();

    await act(async () => root.unmount());
  });

  it('记忆向量面板显示当前模型覆盖并可直接补齐缺失向量', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({
      eligible: 6,
      current: 2,
      queued: 4,
      pruned: 0,
      jobs: 1,
      external_model_id: 'embedding-3',
    });
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            extraction: {},
            retrieval: { coverage: { eligible: 0, current: 0 } },
            embedding: {
              configured: true,
              external_model_id: 'embedding-3',
              eligible: 6,
              current: 2,
              stale: 0,
              blockers: ['embedding_coverage_incomplete'],
            },
            memory_jobs: [],
          }}
          onRefresh={onRefresh}
        />,
      ),
    );

    expect(container.textContent).toContain('记忆向量');
    expect(container.textContent).toContain('embedding-3');
    expect(container.textContent).toContain('2 / 6 条');
    expect(container.textContent).toContain('不会重新分析笔记');
    const rebuild = Array.from(container.querySelectorAll('button')).find(
      (button) => button.textContent === '生成缺失向量',
    ) as HTMLButtonElement;
    await act(async () => {
      rebuild.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(request).toHaveBeenCalledWith('/memory/embeddings/rebuild', { method: 'POST' });
    expect(onRefresh).toHaveBeenCalledOnce();

    await act(async () => root.unmount());
  });

  it('当前集合页面显示一次提取与原子替换进度', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            extraction: {
              policy: {
                enabled: true,
                source_mode: 'automatic',
              },
              revision: 1,
              readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
            },
            memory_jobs: [{
              id: 'memory-job',
              job_type: 'memory.extract',
              status: 'running',
              updated_at: 1,
              progress: {
                phase: 'extracting_note',
                completed: 2,
                total: 8,
                current_index: 3,
                current_path: 'notes/roadmap.md',
                items_published: 3,
                empty_sets_published: 0,
                source_ingestion_failures: 1,
                source_ingestion_failure_notes: [{
                  index: 2,
                  path: 'notes/binary.md',
                  error_code: 'memory_source_not_utf8',
                }],
                generated_output_failures: 0,
                generated_output_failure_notes: [],
                notes_evaluated: 2,
                source_policy_skipped: 0,
                already_evaluated_skipped: 1,
              },
            }],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('笔记当前记忆集合');
    expect(container.textContent).toContain('一次提取 · 整体替换');
    expect(container.textContent).toContain('每篇笔记一次模型调用');
    expect(container.textContent).toContain('按来源完整集合原子替换');
    expect(container.textContent).not.toContain('两阶段长期记忆');
    expect(container.textContent).not.toContain('候选审核');
    expect(container.textContent).toContain('执行中 · 25%');
    expect(container.textContent).toContain('正在处理第 3 / 8 篇：notes/roadmap.md');
    expect(container.textContent).toContain('发布当前记忆 3 条');
    expect(container.textContent).toContain('未变化且已处理，跳过模型 1 篇');
    const runButton = Array.from(container.querySelectorAll('button')).find((button) => button.textContent?.includes('任务执行中'));
    expect(runButton?.disabled).toBe(true);

    await act(async () => root.unmount());
  });

  it('提取模型未就绪时明确阻塞手动任务', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            extraction: {
              policy: { enabled: true, source_mode: 'automatic', max_evidence_per_note: 3 },
              revision: 1,
              readiness: { ready: false, blockers: ['model_binding_missing'] },
            },
            memory_jobs: [],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('尚未绑定“记忆提取”模型');
    const runButton = Array.from(container.querySelectorAll('button')).find(
      (button) => button.textContent === '处理新增或变化的笔记',
    );
    expect(runButton?.disabled).toBe(true);

    await act(async () => root.unmount());
  });

  it('旧版候选不会重新出现在当前记忆页面', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            candidates: [{
              id: 'candidate-1',
              source_path: 'notes/article.md',
              candidate: { content: '旧版过宽候选' },
              confidence: 0.9,
              importance: 0.8,
            }],
            extraction: {
              policy: {
                enabled: true,
                source_mode: 'automatic',
              },
              revision: 1,
              readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
            },
            memory_jobs: [],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('笔记当前记忆集合');
    expect(container.querySelectorAll('.record-item').length).toBe(0);

    await act(async () => root.unmount());
  });

  it('已有记忆任务时禁用重复提交', async () => {
    const request = vi.spyOn(adminApi, 'request');
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            extraction: {
              policy: { enabled: true, source_mode: 'automatic', max_evidence_per_note: 3 },
              revision: 1,
              readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
            },
            memory_jobs: [{
              id: 'active-memory-job',
              job_type: 'memory.extract',
              status: 'running',
              progress: { phase: 'extracting_note', completed: 1, total: 3, current_index: 2 },
            }],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    const active = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('任务执行中'),
    );
    expect(active?.disabled).toBe(true);
    expect(container.textContent).toContain('按来源完整集合原子替换');
    expect(request).not.toHaveBeenCalled();

    await act(async () => root.unmount());
  });

  it('可明确重新提取全部已经处理的笔记', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({
      id: 'forced-memory-job',
      job_type: 'memory.extract',
      status: 'queued',
      admission: 'queued',
    });
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{
            memories: [],
            extraction: {
              policy: { enabled: true, source_mode: 'automatic', max_evidence_per_note: 3 },
              revision: 1,
              readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
            },
            memory_jobs: [],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    const run = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('重新提取全部笔记'),
    ) as HTMLButtonElement;
    await act(async () => {
      run.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(confirm.mock.calls[0][0]).toContain('再次调用一次提取模型');
    expect(request).toHaveBeenCalledWith('/memory/extraction/run', {
      method: 'POST',
      body: { include_evaluated: true },
    });
    expect(container.textContent).toContain('已开始重新提取全部笔记');

    await act(async () => root.unmount());
  });

  it('当前记忆无需父页面刷新即可连续删除多条记录', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({});
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    const memory = {
      id: 'memory-1',
      content: 'Admin 登录认证始终保留。',
      memory_type: 'decision',
      ownership: 'explicit',
      canonical_path: '_mcp-vault/memory/current/explicit/memory-1.md',
      updated_at: 1,
      revision: 7,
      sources: [{
        source_type: 'note',
        path: 'notes/security.md',
        file_id: 'file-1',
        revision: 4,
        heading: ['Admin'],
        start_line: 12,
        end_line: 14,
      }],
    };
    const secondMemory = {
      ...memory,
      id: 'memory-2',
      content: '第二条长期记忆。',
      canonical_path: '_mcp-vault/memory/current/explicit/memory-2.md',
      revision: 9,
    };
    const extraction = {
      policy: { enabled: false, source_mode: 'automatic' },
      readiness: { ready: false, blockers: ['extraction_disabled'] },
    };

    await act(async () =>
      root.render(
        <ManagementPage
          page="memory"
          data={{ memories: [memory, secondMemory], extraction, memory_jobs: [] }}
          onRefresh={onRefresh}
        />,
      ),
    );

    expect(container.textContent).toContain('查看来源笔记与证据定位（1）');
    expect(container.textContent).toContain(
      '规范文件 _mcp-vault/memory/current/explicit/memory-1.md',
    );
    expect(container.textContent).toContain('notes/security.md');
    expect(container.textContent).toContain('修订 4');
    expect(container.textContent).toContain('第 12–14 行');
    expect(container.textContent).toContain('原文仍保留在对应笔记及其修订历史中');

    const remove = container.querySelector(
      'button[aria-label="删除当前记忆 memory-1"]',
    ) as HTMLButtonElement;
    await act(async () => {
      remove.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('当前规范 Markdown 和记忆投影都会删除');
    expect(request).toHaveBeenCalledWith('/memories/memory-1?expected_revision=7', {
      method: 'DELETE',
    });

    expect(container.querySelector('button[aria-label="删除当前记忆 memory-1"]')).toBeNull();
    expect(container.textContent).toContain('长期记忆（1）');
    const removeSecond = container.querySelector(
      'button[aria-label="删除当前记忆 memory-2"]',
    ) as HTMLButtonElement;
    expect(removeSecond).not.toBeNull();
    await act(async () => {
      removeSecond.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(request).toHaveBeenCalledWith('/memories/memory-2?expected_revision=9', {
      method: 'DELETE',
    });
    expect(container.querySelector('button[aria-label="删除当前记忆 memory-2"]')).toBeNull();
    expect(container.textContent).toContain('长期记忆（0）');
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(onRefresh).toHaveBeenCalledTimes(2);

    await act(async () => root.unmount());
  });
});
