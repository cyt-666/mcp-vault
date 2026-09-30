import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { adminApi } from './api';
import { SemanticMemoryPage } from './semantic-memory';

function click(container: HTMLElement, text: string) {
  const button = [...container.querySelectorAll('button')].find((candidate) => candidate.textContent?.includes(text));
  if (!button) throw new Error(`button not found: ${text}`);
  button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
}

function setInput(container: HTMLElement, label: string, value: string) {
  const input = container.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
  if (!input) throw new Error(`input not found: ${label}`);
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set?.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new Event('change', { bubbles: true }));
}

function setTextarea(container: HTMLElement, label: string, value: string) {
  const textarea = container.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`);
  if (!textarea) throw new Error(`textarea not found: ${label}`);
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set?.call(textarea, value);
  textarea.dispatchEvent(new Event('input', { bubbles: true }));
  textarea.dispatchEvent(new Event('change', { bubbles: true }));
}

const data = {
  cards: [{
    card_id: 'card-1', title: 'WebDAV 冲突', kind: 'decision', scope_ref: 'project',
    assertion_status: 'source_asserted', revision_number: 4,
    items: [{ ordinal: 0, kind: 'decision', content: '先读取当前 revision。', observation_id: 'obs-1', conditions: ['存在当前 revision'], exceptions: ['恢复期间暂停'], ordered_steps: ['读取', '比较'], unresolved_items: ['Provider 质量待验'], evidence_ref_ids: ['evidence-1'] }],
    source_bindings: [{ source_id: 'source-1', source_revision_id: 'source-rev-1', evidence_ref_id: 'evidence-1' }],
  }],
  composed_cards: [{
    composed_card_id: 'composed-1', title: '组合约束', kind: 'constraint', scope_ref: 'project',
    assertion_status: 'source_asserted', revision_number: 2, items: [], source_bindings: [],
  }],
  status: { extraction_pending_count: 0, organization_jobs: [], rules_revision: 7 },
};

describe('SemanticMemoryPage', () => {
  beforeEach(() => vi.restoreAllMocks());

  it('renders qualifiers, composed label and evidence spans without full note body', async () => {
    vi.spyOn(adminApi, 'request').mockResolvedValue({
      evidence: {
        evidence_ref_id: 'evidence-1', source_id: 'source-1', source_revision_id: 'source-rev-1',
        body_spans: [{ start_byte: 4, end_byte: 19, text: '先读取当前 revision。' }],
        context_spans: [{ start_byte: 0, end_byte: 3, text: '# 冲突' }],
      },
    });
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(<SemanticMemoryPage data={data} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(container.textContent).toContain('组合约束');
    expect(container.textContent).toContain('M2 组合卡');
    await act(async () => click(container, 'WebDAV 冲突'));
    expect(container.textContent).toContain('存在当前 revision');
    await act(async () => click(container, '展开 evidence evidence-1'));
    expect(container.textContent).toContain('4..19');
    expect(container.textContent).toContain('先读取当前 revision。');
    expect(container.textContent).not.toContain('PRIVATE-FULL-NOTE-BODY');
    await act(async () => root.unmount());
  });

  it('shows no-answer gaps and sends revision/idempotency without actor or vault_id', async () => {
    const request = vi.spyOn(adminApi, 'request').mockImplementation(async (path) => {
      if (path === '/semantic/pack') return { current_context: [], relevant_experiences: [], evidence_gaps: [{ code: 'source_unavailable' }], diagnostics: ['candidate_changed_before_response'], estimated_tokens: 0 };
      return {};
    });
    const notify = vi.fn();
    const onRefresh = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(<SemanticMemoryPage data={data} notify={notify} onRefresh={onRefresh} />));
    setInput(container, '语义任务', '当前冲突约束');
    await act(async () => click(container, '构建 Pack'));
    expect(container.textContent).toContain('有证据缺口');
    expect(container.textContent).toContain('source_unavailable');
    await act(async () => click(container, 'WebDAV 冲突'));
    setInput(container, '旧断言', '先读取当前 revision。');
    setInput(container, '新断言', '先读取新的 current revision。');
    await act(async () => click(container, 'Correct'));
    const mutation = request.mock.calls.find(([path]) => path === '/semantic/correct');
    expect(mutation).toBeDefined();
    const body = mutation?.[1]?.body as Record<string, unknown>;
    expect(body.expected_parent_revision).toBe(4);
    expect(body.expected_rules_revision).toBe(7);
    expect(typeof body.idempotency_key).toBe('string');
    expect(body).not.toHaveProperty('actor');
    expect(body).not.toHaveProperty('vault_id');
    expect(onRefresh).toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it('keeps explicit authorized memory separate and sends a stable key', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({
      explicit: {
        outcome: 'stored',
        memory: {
          memory_id: 'explicit-1', ownership: 'explicit', revision: 1,
          content: '用户授权的原文', embedding_eligible: true, embedding_binding_present: false,
        },
      },
    });
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(<SemanticMemoryPage data={data} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(container.textContent).toContain('用户授权原文记忆');
    expect(container.textContent).toContain('独立 explicit memory');
    setTextarea(container, '授权原文记忆', '用户授权的原文');
    await act(async () => click(container, '保存原文记忆'));
    const mutation = request.mock.calls.find(([path]) => path === '/semantic/remember-explicit');
    expect(mutation).toBeDefined();
    const body = mutation?.[1]?.body as Record<string, unknown>;
    expect(body.content).toBe('用户授权的原文');
    expect(typeof body.idempotency_key).toBe('string');
    expect(String(body.idempotency_key).length).toBeGreaterThan(0);
    expect(body).not.toHaveProperty('actor');
    expect(body).not.toHaveProperty('vault_id');
    expect(container.textContent).toContain('ownership');
    expect(container.textContent).toContain('未配置（不外发）');
    expect(request.mock.calls.some(([path]) => path === '/semantic/pack')).toBe(false);
    const firstKey = body.idempotency_key;
    setTextarea(container, '授权原文记忆', '第二段用户授权原文');
    await act(async () => click(container, '保存原文记忆'));
    const secondCall = request.mock.calls.filter(([path]) => path === '/semantic/remember-explicit')[1];
    const secondBody = secondCall?.[1]?.body as Record<string, unknown>;
    expect(secondBody.content).toBe('第二段用户授权原文');
    expect(secondBody.idempotency_key).not.toBe(firstKey);
    await act(async () => root.unmount());
  });

  it('retries a rejected explicit save with the same payload and key', async () => {
    let calls = 0;
    const request = vi.spyOn(adminApi, 'request').mockImplementation(async (path) => {
      if (path !== '/semantic/remember-explicit') return {};
      calls += 1;
      if (calls === 1) throw new Error('temporary save failure');
      return {
        explicit: {
          outcome: 'stored',
          memory: {
            memory_id: 'explicit-retry', ownership: 'explicit', revision: 1,
            content: 'retry body', embedding_eligible: true, embedding_binding_present: false,
          },
        },
      };
    });
    const notify = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(<SemanticMemoryPage data={data} notify={notify} onRefresh={vi.fn()} />));
    setTextarea(container, '授权原文记忆', 'retry body');
    await act(async () => click(container, '保存原文记忆'));
    expect(notify).toHaveBeenCalledWith('temporary save failure', 'danger');
    const textarea = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="授权原文记忆"]');
    expect(textarea?.value).toBe('retry body');
    const firstBody = request.mock.calls[0]?.[1]?.body as Record<string, unknown>;
    await act(async () => click(container, '保存原文记忆'));
    const secondBody = request.mock.calls[1]?.[1]?.body as Record<string, unknown>;
    expect(secondBody).toEqual(firstBody);
    expect(textarea?.value).toBe('');
    expect(container.textContent).toContain('explicit-retry');
    await act(async () => root.unmount());
  });

  it('prevents duplicate explicit saves while the request is pending', async () => {
    let resolveRequest!: (value: unknown) => void;
    const pending = new Promise((resolve) => { resolveRequest = resolve; });
    const request = vi.spyOn(adminApi, 'request').mockImplementation((path) => {
      if (path === '/semantic/remember-explicit') return pending;
      return Promise.resolve({});
    });
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(<SemanticMemoryPage data={data} notify={vi.fn()} onRefresh={vi.fn()} />));
    setTextarea(container, '授权原文记忆', 'pending body');
    const button = [...container.querySelectorAll('button')].find((candidate) => candidate.textContent?.includes('保存原文记忆')) as HTMLButtonElement;
    if (!button) throw new Error('explicit save button not found');
    await act(async () => {
      button.click();
      await Promise.resolve();
    });
    expect(button.disabled).toBe(true);
    button.click();
    expect(request.mock.calls.filter(([path]) => path === '/semantic/remember-explicit')).toHaveLength(1);
    resolveRequest({
      explicit: {
        outcome: 'stored',
        memory: {
          memory_id: 'explicit-pending', ownership: 'explicit', revision: 1,
          content: 'pending body', embedding_eligible: true, embedding_binding_present: false,
        },
      },
    });
    await act(async () => { await pending; });
    const refreshedButton = [...container.querySelectorAll('button')].find((candidate) => candidate.textContent?.includes('保存原文记忆')) as HTMLButtonElement;
    expect(refreshedButton?.textContent).toContain('保存原文记忆');
    expect(container.textContent).toContain('explicit-pending');
    await act(async () => root.unmount());
  });
});
