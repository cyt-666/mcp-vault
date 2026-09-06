import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { adminApi, AdminApiError } from './api';
import { MemoryManagement, MemoryEditor } from './memory-management';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
function input(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, 'value')?.set?.call(element, value);
  element.dispatchEvent(new Event('input', { bubbles: true }));
}
function button(container: HTMLElement, label: string) { return [...container.querySelectorAll('button')].find((button) => button.textContent === label)!; }

describe('当前记忆管理闭环', () => {
  beforeEach(() => { vi.restoreAllMocks(); });
  it('预检指纹与确认串提交到执行 API，409 要求重新预检', async () => {
    const request = vi.spyOn(adminApi, 'request').mockImplementation(async (path) => {
      if (path.endsWith('/preflight')) return { report: { legacy_total: 1, safe_explicit: 1 }, preflight_hash: 'sha256:reviewed', required_confirmation: 'MIGRATE_MEMORY_V2_1' };
      throw new AdminApiError(409, 'conflict', 'changed');
    });
    const notify = vi.fn(); const rootNode = document.createElement('div'); const root = createRoot(rootNode);
    await act(async () => root.render(<MemoryManagement data={{ vault_slug: 'a' }} notify={notify} onRefresh={vi.fn()} />));
    await act(async () => button(rootNode, '迁移预检').click());
    expect(rootNode.textContent).toContain('sha256:reviewed');
    await act(async () => input(rootNode.querySelector('input')!, 'MIGRATE_MEMORY_V2_1'));
    await act(async () => rootNode.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    expect(request).toHaveBeenLastCalledWith('/vaults/a/memory/migration/execute', { method: 'POST', body: { preflight_hash: 'sha256:reviewed', confirmation: 'MIGRATE_MEMORY_V2_1' } });
    expect(rootNode.querySelector('form')).toBeNull(); expect(notify).toHaveBeenCalledWith(expect.stringContaining('重新预检'), 'danger');
    await act(async () => root.unmount());
  });
  it('真实迁移响应使用 migration.completed，局部加载失败不禁用预检', async () => {
    vi.spyOn(adminApi, 'request').mockImplementation(async (path) => path.endsWith('/preflight') ? { report: { legacy_total: 1 }, preflight_hash: 'sha256:checked', required_confirmation: 'MIGRATE_MEMORY_V2_1' } : { migration: { completed: true, migrated_explicit: 1 } });
    const notify = vi.fn(); const rootNode = document.createElement('div'); const root = createRoot(rootNode);
    await act(async () => root.render(<MemoryManagement data={{ vault_slug: 'a', load_errors: { embedding: 'temporarily unavailable' } }} notify={notify} onRefresh={vi.fn()} />));
    expect(rootNode.textContent).toContain('embedding 加载失败');
    await act(async () => button(rootNode, '迁移预检').click());
    await act(async () => input(rootNode.querySelector('input')!, 'MIGRATE_MEMORY_V2_1'));
    await act(async () => rootNode.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    expect(notify).toHaveBeenCalledWith('迁移已完成。', 'success');
    await act(async () => root.unmount());
  });
  it('空集合恢复使用来源集合修订，校准按钮只调用真实 run', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({ admitted: true, job_id: 'j' });
    const rootNode = document.createElement('div'); const root = createRoot(rootNode);
    await act(async () => root.render(<MemoryManagement data={{ vault_slug: 'a', calibration: { channels: [{ channel: 'memory', automatic: true, active: false, profile: { external_model_id: 'embed' }, blockers: ['calibration_missing'] }] }, sources: { sources: [{ file_id: 'empty', path: 'empty.md', set_revision: 9, current_item_count: 0, paused: true, restorable: true }] } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(request).not.toHaveBeenCalled(); expect(rootNode.textContent).not.toContain('语义检索已启用');
    await act(async () => button(rootNode, '运行／重试校准').click());
    expect(request).toHaveBeenCalledWith('/vaults/a/memory/semantic-calibration/run', { method: 'POST', body: { channel: 'memory' } });
    await act(async () => button(rootNode, '恢复来源提取').click());
    expect(request).toHaveBeenLastCalledWith('/vaults/a/memory/extraction/sources/empty/resume', { method: 'POST', body: { expected_set_revision: 9 } });
    await act(async () => root.unmount());
  });
  it('显式编辑仅提交修改字段，空标签明确清空，其他元数据不发送 null', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({});
    const rootNode = document.createElement('div'); const root = createRoot(rootNode);
    await act(async () => root.render(<MemoryEditor data={{ vault_slug: 'b' }} memory={{ id: 'm', revision: 7, content: 'old', memory_type: 'fact', tags: ['keep'], entities: ['entity'], importance: 0.8, confidence: 0.7, valid_from: 100 }} notify={vi.fn()} onRefresh={vi.fn()} onClose={vi.fn()} />));
    await act(async () => { input(rootNode.querySelector('textarea')!, 'edited'); input(rootNode.querySelectorAll('input')[1], ''); });
    await act(async () => rootNode.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    expect(request).toHaveBeenCalledWith('/vaults/b/memories/m', { method: 'PATCH', body: { expected_revision: 7, content: 'edited', tags: [] } });
    await act(async () => root.unmount());
  });
  it('51 个暂停来源完整分页，轮询保留尾页，旧 Vault 的迟到响应不进入新页面', async () => {
    const source = (index: number) => ({ file_id: `a-${index}`, path: `a-source-${index}.md`, set_revision: 1, paused: true, restorable: true, current_item_count: 0 });
    const first = Array.from({ length: 50 }, (_, index) => source(index));
    let finish: ((value: unknown) => void) | undefined;
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({ sources: [source(50)], next_offset: null });
    const node = document.createElement('div'); const root = createRoot(node);
    const render = (slug: string, rows: unknown[], next: number | null) => <MemoryManagement key={slug} data={{ vault_slug: slug, sources: { sources: rows, next_offset: next } }} notify={vi.fn()} onRefresh={vi.fn()} />;
    await act(async () => root.render(render('a', first, 50)));
    await act(async () => button(node, '加载更多暂停来源').click());
    expect(node.textContent).toContain('a-source-50.md');
    await act(async () => root.render(render('a', [...first], 50)));
    expect(node.textContent).toContain('a-source-50.md');
    expect(request).toHaveBeenCalledWith('/vaults/a/memory/extraction/sources?paused=true&limit=50&offset=50');
    request.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    await act(async () => root.render(render('a', [...first], 50)));
    await act(async () => root.render(render('b', [{ ...source(0), file_id: 'b', path: 'b-only.md' }], null)));
    await act(async () => finish?.({ sources: [source(50)], next_offset: null }));
    expect(node.textContent).toContain('b-only.md');
    expect(node.textContent).not.toContain('a-source-');
    await act(async () => root.unmount());
  });
});
