import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';
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
    vi.spyOn(adminApi, 'restoreSession').mockResolvedValue(null);
    vi.spyOn(adminApi, 'setupStatus').mockResolvedValue({ setup_available: false });
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
      if (path === '/dashboard') return { ready: true };
      if (path === '/memories?limit=50') return { memories: [] };
      if (path === '/memory/extraction') {
        return {
          policy: { enabled: true, source_mode: 'automatic', max_evidence_per_note: 3 },
          readiness: { ready: true, blockers: [] },
          phase1_readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
          phase2_readiness: { ready: true, blockers: [], external_model_id: 'consolidate-model' },
          stage1: { total: 2, ready: 1, no_output: 0, pending: 1 },
          consolidation: { generation: 0, pipeline_generation: 1 },
        };
      }
      if (path === '/jobs/overview?limit=50') {
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

    expect(request).toHaveBeenCalledWith('/jobs/overview?limit=50');
    expect(container.textContent).toContain('任务执行中 · 50%');
    expect(container.textContent).toContain('older-ru…ry-job');

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
                  raw_memories_staged: 0,
                  phase1_no_output: 0,
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
            ],
            retry_wait: [],
            history: [
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
                    schema_path: '$.evidence',
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
              completed: 2,
              failed: 1,
              cancelled: 0,
              active: 1596,
              terminal: 3,
            },
            truncated: { running: false, queued: true, retry_wait: false },
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('Vault 重新扫描');
    expect(container.textContent).toContain('重建知识索引');
    expect(container.textContent).toContain('正在执行（1）');
    expect(container.textContent).toContain('等待执行（1595）');
    expect(container.textContent).toContain('已结束历史（显示 3 / 共 3）');
    expect(container.textContent).toContain('等待中');
    expect(container.textContent).toContain('进度 25%');
    expect(container.textContent).toContain('进度 100%');
    expect(container.textContent).toContain('阶段一：提取原始记忆');
    expect(container.textContent).toContain('正在处理第 1 / 228 篇：projects/current.md');
    expect(container.textContent).toContain('上次尝试：AI 服务已接受请求，但响应正文读取失败');
    expect(container.textContent).toContain('完成但有失败');
    expect(container.textContent).toContain('源文件无法处理 1 篇（模型未调用）');
    expect(container.textContent).toContain('最近源文件问题 notes/binary.md：笔记不是 UTF-8 文本');
    expect(container.textContent).toContain('模型输出校验失败 1 篇（模型已调用）');
    expect(container.textContent).toContain('最近模型输出问题 notes/bad.md');
    expect(container.textContent).toContain('缺少必填字段 $.evidence');
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
            memory: { active: 0, pending_consolidation: 0 },
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
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('笔记语义召回');
    expect(container.textContent).toContain('50%');
    expect(container.textContent).toContain('110 / 220 个内容分块');
    expect(container.textContent).toContain('仍有笔记分块等待生成向量');

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
    expect(jobErrorLabel('provider_schema_invalid')).toContain('返回了 JSON');
    expect(jobErrorLabel('memory_extract_output_failure_limit')).toContain('连续 3 次');
    expect(jobErrorLabel('memory_source_too_large')).toContain('512 KiB');
    expect(jobErrorLabel('memory_phase1_evidence_anchor_invalid')).toContain('超出笔记范围');
  });

  it('两阶段记忆页面不暴露候选审核并显示可解释的持久化进度', async () => {
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
                enabled: false,
                source_mode: 'automatic',
                max_evidence_per_note: 3,
              },
              revision: null,
              readiness: { ready: false, blockers: ['extraction_disabled', 'consolidation_model_binding_missing'] },
              phase1_readiness: { ready: false, blockers: ['extraction_disabled'], external_model_id: 'extract-model' },
              phase2_readiness: { ready: false, blockers: ['consolidation_model_binding_missing'] },
              stage1: { total: 8, ready: 2, no_output: 1, withdrawn: 0, pending: 2 },
              consolidation: { generation: 3, last_success_at: 1, pipeline_generation: 1, memory_summary_present: true },
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
                raw_memories_staged: 2,
                phase1_no_output: 1,
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

    expect(container.textContent).toContain('两阶段长期记忆');
    expect(container.textContent).toContain('不存在“待审核候选”');
    expect(container.textContent).toContain('不需要添加特殊标记');
    expect(container.textContent).not.toContain('进入审核的最低置信度');
    expect(container.textContent).toContain('每篇最多保留原文证据（1–10）');
    expect(container.textContent).toContain('尚未绑定阶段二“记忆整理”模型');
    expect(container.textContent).toContain('阶段一 · 提取模型');
    expect(container.textContent).toContain('阶段二 · 整理模型');
    expect(container.textContent).toContain('执行中 · 25%');
    expect(container.textContent).toContain('正在处理第 3 / 8 篇：notes/roadmap.md');
    expect(container.textContent).toContain('提炼原始记忆 2 篇');
    expect(container.textContent).toContain('无需形成记忆 1 篇');
    expect(container.textContent).not.toContain('原文证据未通过');
    expect(container.textContent).toContain('未变化且已处理，跳过模型 1 篇');
    const runButton = Array.from(container.querySelectorAll('button')).find((button) => button.textContent?.includes('任务执行中'));
    expect(runButton?.disabled).toBe(true);

    await act(async () => root.unmount());
  });

  it('等待首次全量生成时配置就绪即可手动触发', async () => {
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
              readiness: { ready: false, blockers: ['memory_pipeline_regeneration_pending'] },
              phase1_readiness: { ready: true, blockers: [], external_model_id: 'mimo-v2.5' },
              phase2_readiness: { ready: true, blockers: [], external_model_id: 'mimo-v2.5' },
              stage1: { total: 0, ready: 0, no_output: 0, withdrawn: 0, pending: 0 },
              consolidation: {
                generation: 0,
                pipeline_generation: 2,
                regeneration_pending: true,
              },
            },
            memory_jobs: [],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).toContain('准备重新生成');
    const runButton = Array.from(container.querySelectorAll('button')).find(
      (button) => button.textContent === '立即开始全量生成',
    );
    expect(runButton?.disabled).toBe(false);

    await act(async () => root.unmount());
  });

  it('旧版候选不会重新出现在两阶段记忆页面', async () => {
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
                max_evidence_per_note: 3,
              },
              revision: 1,
              readiness: { ready: true, blockers: [] },
              phase1_readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
              phase2_readiness: { ready: true, blockers: [], external_model_id: 'consolidate-model' },
              stage1: { total: 0, ready: 0, no_output: 0, withdrawn: 0, pending: 0 },
              consolidation: { generation: 0, last_success_at: null, pipeline_generation: 0, memory_summary_present: false },
            },
            memory_jobs: [{
              id: 'reset-job',
              job_type: 'memory.reset_pipeline',
              status: 'running',
              progress: { phase: 'resetting_memory_pipeline', completed: 0, total: 1 },
            }],
          }}
          onRefresh={() => undefined}
        />,
      ),
    );

    expect(container.textContent).not.toContain('确认写入长期记忆');
    expect(container.querySelectorAll('.record-item').length).toBe(0);
    expect(container.textContent).toContain('旧版记忆系统正在整体作废并清理');
    expect(container.textContent).toContain('正在清空旧版记忆系统');

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
              readiness: { ready: true, blockers: [] },
              phase1_readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
              phase2_readiness: { ready: true, blockers: [], external_model_id: 'consolidate-model' },
              stage1: { total: 3, ready: 1, no_output: 2, withdrawn: 0, pending: 1 },
              consolidation: { generation: 1, last_success_at: 1, pipeline_generation: 1, regeneration_pending: true },
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
    expect(container.textContent).toContain('配置就绪后会立即创建全量任务');
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
              readiness: { ready: true, blockers: [] },
              phase1_readiness: { ready: true, blockers: [], external_model_id: 'extract-model' },
              phase2_readiness: { ready: true, blockers: [], external_model_id: 'consolidate-model' },
              stage1: { total: 3, ready: 1, no_output: 2, withdrawn: 0, pending: 0 },
              consolidation: { generation: 1, last_success_at: 1, pipeline_generation: 1 },
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

    expect(confirm.mock.calls[0][0]).toContain('再次调用提取模型');
    expect(request).toHaveBeenCalledWith('/memory/extraction/run', {
      method: 'POST',
      body: { include_evaluated: true },
    });
    expect(container.textContent).toContain('已开始重新提取全部笔记');

    await act(async () => root.unmount());
  });

  it('长期记忆无需父页面刷新即可连续归档、恢复和删除多条记录', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({});
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    const memory = {
      id: 'memory-1',
      content: 'Admin 登录认证始终保留。',
      memory_type: 'decision',
      canonical_path: '_mcp-vault/memory/records/2026/08/memory-1.md',
      confidence: 1,
      importance: 0.9,
      updated_at: 1,
      revision: 7,
      status: 'active',
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
      canonical_path: '_mcp-vault/memory/records/2026/08/memory-2.md',
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

    expect(container.textContent).toContain('查看来源与证据定位（1）');
    expect(container.textContent).toContain('notes/security.md');
    expect(container.textContent).toContain('修订 4');
    expect(container.textContent).toContain('第 12–14 行');
    expect(container.textContent).toContain('原文仍保留在对应笔记及其修订历史中');

    const archive = container.querySelector(
      'button[aria-label="归档长期记忆 memory-1"]',
    ) as HTMLButtonElement;
    await act(async () => {
      archive.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(request).toHaveBeenCalledWith('/memories/memory-1/archive', {
      method: 'POST',
      body: { expected_revision: 7 },
    });

    const restore = container.querySelector(
      'button[aria-label="恢复长期记忆 memory-1"]',
    ) as HTMLButtonElement;
    expect(restore).not.toBeNull();
    await act(async () => {
      restore.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(request).toHaveBeenCalledWith('/memories/memory-1/restore', {
      method: 'POST',
      body: { expected_revision: 8 },
    });

    const remove = container.querySelector(
      'button[aria-label="永久删除长期记忆 memory-1"]',
    ) as HTMLButtonElement;
    await act(async () => {
      remove.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0][0]).toContain('当前规范 Markdown 和记忆投影会删除');
    expect(request).toHaveBeenCalledWith('/memories/memory-1?expected_revision=9', {
      method: 'DELETE',
    });

    expect(container.querySelector('button[aria-label="永久删除长期记忆 memory-1"]')).toBeNull();
    expect(container.textContent).toContain('长期记忆（1）');
    const removeSecond = container.querySelector(
      'button[aria-label="永久删除长期记忆 memory-2"]',
    ) as HTMLButtonElement;
    expect(removeSecond).not.toBeNull();
    await act(async () => {
      removeSecond.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(request).toHaveBeenCalledWith('/memories/memory-2?expected_revision=9', {
      method: 'DELETE',
    });
    expect(container.querySelector('button[aria-label="永久删除长期记忆 memory-2"]')).toBeNull();
    expect(container.textContent).toContain('长期记忆（0）');
    expect(onRefresh).toHaveBeenCalledTimes(4);

    await act(async () => root.unmount());
  });
});
