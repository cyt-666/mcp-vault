import { useEffect, useState } from 'react';

import { adminApi } from './api';
import { MemoryEditor } from './memory-management';
import {
  EmptyState,
  Panel,
  StatusBadge,
  type NoticeTone,
} from './ui';
import {
  arrayRecords,
  asRecord,
  formatRequestError,
  memoryTypeLabel,
  numberValue,
  stringValue,
  type JsonObject,
} from './view-model';

type Notify = (message: string, tone?: NoticeTone) => void;

function idempotencyKey(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `admin-raw-memory-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function RawExplicitMemoryPage({
  data,
  notify,
  onRefresh,
}: {
  data: JsonObject | null;
  notify: Notify;
  onRefresh: () => void;
}) {
  const [memories, setMemories] = useState(() => arrayRecords(data?.memories));
  const [content, setContent] = useState('');
  const [kind, setKind] = useState('');
  const [editing, setEditing] = useState<JsonObject | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => setMemories(arrayRecords(data?.memories)), [data?.memories]);

  async function create() {
    if (!content.trim()) return;
    setBusy(true);
    try {
      const result = asRecord(await adminApi.request('/memories', {
        method: 'POST',
        body: { content: content.trim(), kind: kind || null, idempotency_key: idempotencyKey() },
      }));
      const memory = asRecord(result.memory);
      if (Object.keys(memory).length > 0) setMemories((current) => [memory, ...current]);
      setContent('');
      setKind('');
      notify('显式记忆已保存。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  async function remove(memory: JsonObject) {
    const id = stringValue(memory.id);
    const revision = numberValue(memory.revision);
    if (!id || revision <= 0 || !window.confirm('确定删除这条显式记忆吗？')) return;
    setBusy(true);
    try {
      await adminApi.request(`/memories/${encodeURIComponent(id)}?expected_revision=${revision}`, { method: 'DELETE' });
      setMemories((current) => current.filter((item) => stringValue(item.id) !== id));
      notify('显式记忆已删除。');
      onRefresh();
    } catch (error: unknown) {
      notify(formatRequestError(error), 'danger');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="page-stack">
      <Panel title="显式记忆" eyebrow="授权原文管理" description="这里只管理明确提交的 raw memory；正文按提交内容保存，不经过自动提取或生成。">
        <form className="compact-form" onSubmit={(event) => { event.preventDefault(); void create(); }}>
          <label>内容<textarea required rows={4} value={content} onChange={(event) => setContent(event.target.value)} /></label>
          <label>类型（可选）
            <select value={kind} onChange={(event) => setKind(event.target.value)}>
              <option value="">不指定</option>
              <option value="experience">实践经验</option>
              <option value="decision">决定</option>
              <option value="preference">偏好</option>
              <option value="constraint">约束</option>
              <option value="state">当前状态</option>
              <option value="procedure">操作流程</option>
            </select>
          </label>
          <button className="primary-button" type="submit" disabled={busy || !content.trim()}>{busy ? '正在保存…' : '保存显式记忆'}</button>
        </form>
      </Panel>
      <Panel title={`已保存的显式记忆（${memories.length}）`} description="仅显示授权的显式记忆；语义卡片和来源证据请前往“语义记忆”。">
        {memories.length === 0 ? <EmptyState title="还没有显式记忆" detail="通过上面的表单或 Agent 的明确记忆操作保存内容。" /> : (
          <div className="record-list">
            {memories.map((memory) => (
              <article className="record-item record-item--stack" key={stringValue(memory.id)}>
                <div className="record-title"><strong className="memory-content">{stringValue(memory.content, '无内容')}</strong><StatusBadge tone="success">显式</StatusBadge></div>
                <p>{memory.memory_type ? memoryTypeLabel(memory.memory_type) : '未指定类型'} · 当前修订 {numberValue(memory.revision)}</p>
                <div className="button-row">
                  <button className="secondary-button" type="button" disabled={busy} onClick={() => setEditing(memory)}>编辑</button>
                  <button className="danger-button" type="button" disabled={busy} onClick={() => void remove(memory)}>删除</button>
                </div>
              </article>
            ))}
          </div>
        )}
      </Panel>
      {editing ? <Panel title="编辑显式记忆"><MemoryEditor memory={editing} data={data} notify={notify} onRefresh={onRefresh} onClose={() => setEditing(null)} /></Panel> : null}
    </div>
  );
}
