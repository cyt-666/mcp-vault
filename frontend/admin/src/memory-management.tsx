import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { adminApi, AdminApiError } from './api';
import { Notice, Panel } from './ui';
import type { NoticeTone } from './ui';
import { arrayRecords, asRecord, booleanValue, formatRequestError, numberValue, stringValue } from './view-model';
import type { JsonObject } from './view-model';

type Notify = (message: string, tone?: NoticeTone) => void;
type Props = { data: JsonObject | null; notify: Notify; onRefresh: () => void };
const pathFor = (data: JsonObject | null, path: string) => typeof data?.vault_slug === 'string'
  ? `/vaults/${encodeURIComponent(data.vault_slug)}${path}` : path;

export function MemoryManagement({ data, notify, onRefresh }: Props) {
  const [busy, setBusy] = useState('');
  const [sources, setSources] = useState(() => arrayRecords(asRecord(data?.sources).sources));
  const [next, setNext] = useState<unknown>(asRecord(data?.sources).next_offset);
  const errors = asRecord(data?.load_errors);
  const loadedSourcePages = useRef(1);
  const vaultSlug = stringValue(data?.vault_slug, '');
  useEffect(() => {
    let cancelled = false;
    const first = arrayRecords(asRecord(data?.sources).sources);
    if (loadedSourcePages.current === 1) {
      setSources(first); setNext(asRecord(data?.sources).next_offset); return;
    }
    const prefix = vaultSlug ? `/vaults/${encodeURIComponent(vaultSlug)}` : '';
    void Promise.all(Array.from({ length: loadedSourcePages.current - 1 }, (_, index) => adminApi.request<JsonObject>(`${prefix}/memory/extraction/sources?paused=true&limit=50&offset=${(index + 1) * 50}`))).then((pages) => {
      if (cancelled) return;
      setSources([first, ...pages.map((page) => arrayRecords(page.sources))].flat());
      setNext(pages.at(-1)?.next_offset);
    }).catch(() => { /* Retain loaded rows when a refresh is temporarily unavailable. */ });
    return () => { cancelled = true; };
  }, [data?.sources, vaultSlug]);

  async function action(key: string, operation: () => Promise<void>) {
    setBusy(key);
    try { await operation(); } catch (error) {
      notify(formatRequestError(error), 'danger');
    } finally { setBusy(''); }
  }

  return <>
    {Object.entries(errors).map(([name, error]) => <Notice key={name} tone="warning">{name} 加载失败：{String(error)}。其他管理操作仍可使用。</Notice>)}
    <Panel title="暂停的来源" eyebrow="包含空集合" description="删除最后一条记忆后，来源仍在这里。只有明确恢复操作才会解除暂停并排队提取。">
      {sources.length === 0 ? <p>当前页没有暂停来源。</p> : sources.map((source) => <article className="record-item" key={stringValue(source.file_id)}>
        <div><strong>{stringValue(source.path)}</strong><p>集合修订 {numberValue(source.set_revision)} · 当前记忆 {numberValue(source.current_item_count)} 条</p>
          {!booleanValue(source.restorable) ? <small>原笔记已不存在，无法恢复提取。</small> : null}</div>
        <button type="button" className="secondary-button" disabled={!!busy || !booleanValue(source.restorable)} onClick={() => {
          if (!window.confirm(`恢复 ${stringValue(source.path)} 的自动提取并重新生成当前集合？这可能调用已配置的提取模型。`)) return;
          void action('resume', async () => {
            await adminApi.request(pathFor(data, `/memory/extraction/sources/${encodeURIComponent(stringValue(source.file_id))}/resume`), { method: 'POST', body: { expected_set_revision: numberValue(source.set_revision) } });
            notify('来源恢复已提交，后台完成后自动提取。'); onRefresh();
          });
        }}>恢复来源提取</button>
      </article>)}
      {typeof next === 'number' ? <button type="button" className="secondary-button" disabled={!!busy} onClick={() => void action('sources', async () => {
        const result = asRecord(await adminApi.request(pathFor(data, `/memory/extraction/sources?paused=true&limit=50&offset=${next}`)));
        loadedSourcePages.current += 1;
        setSources((current) => [...current, ...arrayRecords(result.sources)]); setNext(result.next_offset);
      })}>加载更多暂停来源</button> : null}
    </Panel>
  </>;
}

export function MemoryEditor({ memory, data, notify, onRefresh, onClose }: Props & { memory: JsonObject; onClose: () => void }) {
  const [content, setContent] = useState(stringValue(memory.content, ''));
  const [kind, setKind] = useState(stringValue(memory.memory_type, ''));
  const [tags, setTags] = useState(Array.isArray(memory.tags) ? memory.tags.join(', ') : '');
  const [entities, setEntities] = useState(Array.isArray(memory.entities) ? memory.entities.join(', ') : '');
  const [importance, setImportance] = useState(memory.importance == null ? '' : String(memory.importance));
  const [confidence, setConfidence] = useState(memory.confidence == null ? '' : String(memory.confidence));
  const [busy, setBusy] = useState(false);
  async function save(event: FormEvent) {
    event.preventDefault(); setBusy(true);
    const body: JsonObject = { expected_revision: memory.revision };
    const fields: JsonObject = { content, memory_type: kind || null, tags: tags.split(',').map((value) => value.trim()).filter(Boolean), entities: entities.split(',').map((value) => value.trim()).filter(Boolean), importance: importance === '' ? null : Number(importance), confidence: confidence === '' ? null : Number(confidence) };
    for (const [key, value] of Object.entries(fields)) {
      if (JSON.stringify(value) !== JSON.stringify(memory[key] ?? (Array.isArray(value) ? [] : null))) body[key] = value;
    }
    try {
      await adminApi.request(pathFor(data, `/memories/${encodeURIComponent(stringValue(memory.id))}`), { method: 'PATCH', body });
      notify('显式记忆已更新，未修改的元数据保持原值。'); onClose(); onRefresh();
    } catch (error) { notify(error instanceof AdminApiError && error.status === 409 ? '记忆已被修改，请刷新并重新确认；本次没有覆盖新版本。' : formatRequestError(error), 'danger'); } finally { setBusy(false); }
  }
  return <form className="compact-form" onSubmit={(event) => void save(event)} aria-label="编辑显式记忆">
    <label>编辑内容<textarea required value={content} onChange={(event) => setContent(event.target.value)} /></label>
    <label>编辑类型（留空清除）<input value={kind} onChange={(event) => setKind(event.target.value)} /></label>
    <label>编辑标签（逗号分隔，留空清除）<input value={tags} onChange={(event) => setTags(event.target.value)} /></label>
    <label>编辑实体（逗号分隔，留空清除）<input value={entities} onChange={(event) => setEntities(event.target.value)} /></label>
    <label>重要性（留空清除）<input type="number" min="0" max="1" step="0.01" value={importance} onChange={(event) => setImportance(event.target.value)} /></label>
    <label>置信度（留空清除）<input type="number" min="0" max="1" step="0.01" value={confidence} onChange={(event) => setConfidence(event.target.value)} /></label>
    <div className="button-row"><button className="primary-button" disabled={busy || !content.trim()} type="submit">保存修改</button><button className="secondary-button" disabled={busy} type="button" onClick={onClose}>取消编辑</button></div>
  </form>;
}
