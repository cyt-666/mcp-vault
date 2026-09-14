import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { adminApi } from './api';
import { MemoryManagement, MemoryEditor } from './memory-management';
import { MemoryInitializationPanel } from './pages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
function input(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, 'value')?.set?.call(element, value);
  element.dispatchEvent(new Event('input', { bubbles: true }));
}
function button(container: HTMLElement, label: string) { return [...container.querySelectorAll('button')].find((button) => button.textContent === label)!; }

describe('当前记忆管理闭环', () => {
  beforeEach(() => { vi.restoreAllMocks(); });
  it('空集合恢复使用来源集合修订且没有旧校准操作', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({ admitted: true, job_id: 'j' });
    const rootNode = document.createElement('div'); const root = createRoot(rootNode);
    await act(async () => root.render(<MemoryManagement data={{ vault_slug: 'a', calibration: { channels: [{ channel: 'memory', automatic: true, active: false, profile: { external_model_id: 'embed' }, blockers: ['calibration_missing'] }] }, sources: { sources: [{ file_id: 'empty', path: 'empty.md', set_revision: 9, current_item_count: 0, paused: true, restorable: true }] } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(request).not.toHaveBeenCalled(); expect(rootNode.textContent).not.toContain('语义检索已启用');
    expect(rootNode.textContent).not.toContain('运行／重试评测');
    await act(async () => button(rootNode, '恢复来源提取').click());
    expect(request).toHaveBeenLastCalledWith('/vaults/a/memory/extraction/sources/empty/resume', { method: 'POST', body: { expected_set_revision: 9 } });
    await act(async () => root.unmount());
  });
  it('显式编辑仅提交修改字段，空标签明确清空，其他元数据不发送 null', async () => {
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({});
    const rootNode = document.createElement('div'); const root = createRoot(rootNode);
    await act(async () => root.render(<MemoryEditor data={{ vault_slug: 'b' }} memory={{ id: 'm', revision: 7, content: 'old', memory_type: 'experience', tags: ['keep'], entities: ['entity'], importance: 0.8, confidence: 0.7, valid_from: 100 }} notify={vi.fn()} onRefresh={vi.fn()} onClose={vi.fn()} />));
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

  it('新 Vault 已就绪时不显示旧记忆清理入口', async () => {
    const request = vi.spyOn(adminApi, 'request');
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="fresh" initialStatus={{ initialization: null, preview: { phase: 'ready' }, task: null }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).not.toContain('旧记忆一次性初始化');
    expect(request).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it('确认后接受 202 任务，按持久 manifest 显示进度并在 ready 刷新父页面', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    vi.useFakeTimers();
    const onRefresh = vi.fn();
    const request = vi.spyOn(adminApi, 'request');
    request.mockResolvedValueOnce({ task: { state: 'queued', task_id: 'task-1', resumable: false } });
    request.mockResolvedValueOnce({ initialization: { phase: 'clearing', manifest: { files: [{ path: 'old.md' }, { path: 'old-2.md' }], completed_files: 1, file_count: 2 } }, preview: { available: false, error_code: 'maintenance' }, task: { state: 'running', completed_files: 1, total_files: 2 } });
    request.mockResolvedValueOnce({ initialization: { phase: 'ready', manifest: { files: [{ path: 'old.md' }, { path: 'old-2.md' }], completed_files: 2, file_count: 2 } }, preview: { phase: 'ready' }, task: { state: 'ready' } });
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={{ initialization: { phase: 'required' }, preview: { available: true, record_counts: { legacy: 3 }, files: [{ path: 'old.md' }] }, task: null }} notify={vi.fn()} onRefresh={onRefresh} />));
    await act(async () => button(node, '确认清理旧记忆').click());
    expect(request).toHaveBeenLastCalledWith('/memory/initialization/start', { method: 'POST', body: { confirm_discard_legacy_memory: true } });
    await act(async () => { vi.advanceTimersByTime(1500); await Promise.resolve(); });
    expect(node.textContent).toContain('1 / 2 个受管文件');
    await act(async () => { vi.advanceTimersByTime(1500); await Promise.resolve(); });
    expect(onRefresh).toHaveBeenCalled();
    await act(async () => root.unmount());
    vi.useRealTimers();
  });

  it('失败且可续跑时只显示续跑；不可续跑时不提供续跑按钮', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const request = vi.spyOn(adminApi, 'request').mockResolvedValue({ task: { state: 'queued' } });
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={{ initialization: { phase: 'failed', manifest: { files: [] } }, preview: { available: false }, task: { state: 'failed', resumable: true } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).toContain('确认继续初始化');
    await act(async () => button(node, '确认继续初始化').click());
    expect(request).toHaveBeenCalledWith('/memory/initialization/resume', { method: 'POST', body: { confirm_discard_legacy_memory: true } });
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={{ initialization: { phase: 'failed', manifest: { files: [] } }, preview: { available: false }, task: { state: 'failed', resumable: false } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).not.toContain('确认继续初始化');
    await act(async () => root.unmount());
  });

  it('预览不可用时显示未知记录数和持久 manifest 文件数', async () => {
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={{ initialization: { phase: 'clearing', manifest: { files: [{ path: 'a.md' }, { path: 'b.md' }], completed_files: 1, file_count: 2 } }, preview: { available: false, error_code: 'maintenance' }, task: { state: 'running' } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).toContain('旧记录数量暂不可读');
    expect(node.textContent).toContain('受管旧文件已记录 2');
    expect(node.textContent).toContain('1 / 2 个受管文件');
    await act(async () => root.unmount());
  });

  it('切换 Vault 后忽略旧初始化请求的迟到响应', async () => {
    const resolvers: Array<(value: unknown) => void> = [];
    vi.spyOn(adminApi, 'request').mockImplementation(() => new Promise((resolve) => { resolvers.push(resolve); }));
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" notify={vi.fn()} onRefresh={vi.fn()} />));
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="b" notify={vi.fn()} onRefresh={vi.fn()} />));
    await act(async () => resolvers[1]?.({ initialization: { phase: 'required' }, preview: { available: true, record_counts: { legacy: 1 }, files: [] }, task: null }));
    await act(async () => resolvers[0]?.({ initialization: { phase: 'clearing' }, preview: { available: true, record_counts: { legacy: 99 }, files: [] }, task: null }));
    expect(node.textContent).not.toContain('99');
    await act(async () => root.unmount());
  });

  it('切换 Vault 后忽略旧确认请求的迟到响应', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    let resolvePost: ((value: unknown) => void) | undefined;
    const request = vi.spyOn(adminApi, 'request').mockImplementation((path) => {
      if (path.endsWith('/start')) return new Promise((resolve) => { resolvePost = resolve; });
      return Promise.resolve({});
    });
    const node = document.createElement('div'); const root = createRoot(node);
    const status = { initialization: { phase: 'required' }, preview: { available: true, record_counts: { legacy: 1 }, files: [] }, task: null };
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={status} notify={vi.fn()} onRefresh={vi.fn()} />));
    await act(async () => button(node, '确认清理旧记忆').click());
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="b" initialStatus={status} notify={vi.fn()} onRefresh={vi.fn()} />));
    await act(async () => resolvePost?.({ task: { state: 'failed', resumable: true } }));
    expect(request).toHaveBeenCalledWith('/memory/initialization/start', { method: 'POST', body: { confirm_discard_legacy_memory: true } });
    expect(node.textContent).not.toContain('确认继续初始化');
    expect((button(node, '确认清理旧记忆') as HTMLButtonElement).disabled).toBe(false);
    await act(async () => root.unmount());
  });

  it('失败任务展示阶段、相对路径、来源错误码，并区分维护期间不可用的预览', async () => {
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={{ initialization: { phase: 'failed', manifest: { files: [] } }, preview: { available: false, error_code: 'maintenance' }, task: { state: 'failed', error_code: 'cleanup_failed', error_stage: 'retire_legacy', error_path: 'old-memory/a.md', error_source_code: 'storage_permission_denied', journal_summary: [{ operation: 'retire', state: 'failed', count: 1 }] } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).toContain('retire_legacy');
    expect(node.textContent).toContain('old-memory/a.md');
    expect(node.textContent).toContain('storage_permission_denied');
    expect(node.textContent).toContain('预览因当前维护状态暂不可读');
    expect(node.textContent).toContain('retire：failed（1）');
    await act(async () => root.unmount());
  });

  it('旧任务没有细诊断字段时保留错误码和只读日志汇总，不伪造阶段路径', async () => {
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="a" initialStatus={{ initialization: { phase: 'failed', manifest: { files: [] } }, preview: { available: false }, task: { state: 'failed', error_code: 'legacy_cleanup_failed', journal_summary: [{ operation: 'scan', state: 'completed', count: 4 }] } }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).toContain('legacy_cleanup_failed');
    expect(node.textContent).toContain('旧任务没有保存更细的错误阶段、路径或来源错误码');
    expect(node.textContent).toContain('scan：completed（4）');
    expect(node.textContent).not.toContain('old-memory/');
    await act(async () => root.unmount());
  });

  it('初始化完成但 Vault 仍错误时保留可见提示，普通 active fresh ready 仍隐藏', async () => {
    const node = document.createElement('div'); const root = createRoot(node);
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="broken" initialStatus={{ initialization: { phase: 'ready' }, preview: { phase: 'ready' }, task: { state: 'ready' }, vault_status: 'error' }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).toContain('记忆初始化已完成');
    expect(node.textContent).toContain('Vault 仍处于错误状态');
    expect(node.textContent).toContain('Vault 设置');
    await act(async () => root.render(<MemoryInitializationPanel vaultKey="fresh" initialStatus={{ initialization: null, preview: { phase: 'ready' }, task: null, vault_status: 'active' }} notify={vi.fn()} onRefresh={vi.fn()} />));
    expect(node.textContent).not.toContain('记忆初始化已完成');
    await act(async () => root.unmount());
  });
});
