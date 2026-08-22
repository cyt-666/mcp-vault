import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';
import { AdminApiError, adminApi } from './api';
import { ManagementPage } from './pages';
import { formatRequestError } from './view-model';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

describe('Admin 管理界面', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.spyOn(adminApi, 'setupStatus').mockResolvedValue({ setup_available: false });
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
            jobs: [
              {
                id: '019d-test-job',
                job_type: 'vault.reconcile',
                status: 'queued',
                attempts: 0,
                max_attempts: 3,
                progress: 0.25,
              },
            ],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('Vault 重新扫描');
    expect(container.textContent).toContain('等待中');
    expect(container.textContent).toContain('高级：查看原始响应');
    expect((container.querySelector('details.raw-details') as HTMLDetailsElement).open).toBe(false);

    await act(async () => root.unmount());
  });
});
