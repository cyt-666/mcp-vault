import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { adminApi, AdminApiError } from './api';
import { Notice, Panel, RawData, StatusBadge } from './ui';
import type { NoticeTone } from './ui';
import { arrayRecords, asRecord, booleanValue, formatRequestError, numberValue, stringValue } from './view-model';
import type { JsonObject } from './view-model';

type Notify = (message: string, tone?: NoticeTone) => void;
type Props = { data: JsonObject | null; notify: Notify; onRefresh: () => void };
const pathFor = (data: JsonObject | null, path: string) => typeof data?.vault_slug === 'string'
  ? `/vaults/${encodeURIComponent(data.vault_slug)}${path}` : path;

export function MemoryManagement({ data, notify, onRefresh }: Props) {
  const [preflight, setPreflight] = useState<JsonObject | null>(null);
  const [migration, setMigration] = useState<JsonObject | null>(null);
  const [confirmation, setConfirmation] = useState('');
  const [busy, setBusy] = useState('');
  const [sources, setSources] = useState(() => arrayRecords(asRecord(data?.sources).sources));
  const [next, setNext] = useState<unknown>(asRecord(data?.sources).next_offset);
  const channels = arrayRecords(asRecord(data?.calibration).channels);
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
      if (error instanceof AdminApiError && error.status === 409 && key === 'migrate') {
        setPreflight(null); setConfirmation('');
        notify('数据已变化，请重新预检并核对报告后再执行迁移。', 'danger');
      } else notify(formatRequestError(error), 'danger');
    } finally { setBusy(''); }
  }

  return <>
    {Object.entries(errors).map(([name, error]) => <Notice key={name} tone="warning">{name} 加载失败：{String(error)}。其他管理操作仍可使用。</Notice>)}
    <Panel title="检索效果诊断（可选）" eyebrow="内置合成样本" description="评测仅供诊断，不决定语义检索是否启用。启动和模型绑定不会自动评测；手动运行会调用 embedding 模型并产生相应费用。结果不代表真实笔记准确率。">
      {channels.map((channel) => {
        const name = stringValue(channel.channel);
        const profile = asRecord(channel.profile);
        const run = asRecord(channel.run);
        const report = asRecord(channel.report);
        return <article className="record-item record-item--stack" key={name}>
          <strong>{name === 'memory' ? '记忆通道' : '笔记通道'} · {stringValue(profile.external_model_id, '未绑定模型')}</strong>
          <StatusBadge tone={booleanValue(channel.active) ? 'success' : 'neutral'}>{booleanValue(channel.active) ? '内置基准通过' : stringValue(run.status, '尚未评测')}</StatusBadge>
          <p>{Array.isArray(channel.blockers) ? channel.blockers.map(String).join('；') : ''}</p>
          {channel.joint_no_answer ? <details><summary>查看双通道联合无答案检验</summary><RawData data={asRecord(channel.joint_no_answer)} /></details> : null}
          <p>累计请求 {numberValue(run.requests)} / {numberValue(run.request_limit, 32)} · 请求字节 {numberValue(run.request_bytes)} / {numberValue(run.byte_limit, 2097152)}</p>
          <div className="button-row">
            <button type="button" className="secondary-button" disabled={!!busy || !channel.profile || !booleanValue(channel.automatic)} onClick={() => void action(`calibrate-${name}`, async () => {
              const result = asRecord(await adminApi.request(pathFor(data, '/memory/semantic-calibration/run'), { method: 'POST', body: { channel: name } }));
              notify(booleanValue(result.admitted) ? '已提交或复用服务端校准任务。' : '当前配置不允许校准，请检查模型及维护设置。', booleanValue(result.admitted) ? 'success' : 'warning'); onRefresh();
            })}>运行／重试评测</button>
            <button type="button" className="secondary-button" disabled={!!busy} onClick={() => void action('maintenance', async () => {
              await adminApi.request(pathFor(data, '/memory/semantic-calibration/maintenance'), { method: 'PUT', body: { enabled: !booleanValue(channel.automatic) } }); onRefresh();
            })}>{booleanValue(channel.automatic) ? '暂停诊断调用' : '允许诊断调用'}</button>
          </div>
          <small>重试按任务保存的请求预算申请新一轮额度；不会重新生成记忆或重建有效业务向量。暂停仅限制诊断调用，不影响正常检索。</small>
          {Object.keys(report).length > 0 ? <details><summary>查看内置基准评测报告</summary><RawData data={report} /></details> : null}
          {typeof run.report_json === 'string' ? <details><summary>查看最近执行结果</summary><pre className="data-inspector">{run.report_json}</pre></details> : null}
        </article>;
      })}
      {channels.length === 0 ? <Notice tone="info">诊断状态尚未加载；正常检索不依赖此结果。</Notice> : null}
    </Panel>
    <Panel title="旧记忆迁移" eyebrow="需要明确确认" description="升级校准不会迁移旧记忆。先备份数据库与 Vault，再检查来源分类和未解决项目。">
      <button type="button" className="secondary-button" disabled={!!busy} onClick={() => void action('preflight', async () => {
        setPreflight(asRecord(await adminApi.request(pathFor(data, '/memory/migration/preflight'), { method: 'POST' })));
        setMigration(null); setConfirmation('');
      })}>迁移预检</button>
      {preflight ? <>
        <RawData data={asRecord(preflight.report)} />
        <p>预检指纹：<code>{stringValue(preflight.preflight_hash)}</code></p>
        {numberValue(asRecord(preflight.report).legacy_total) === 0 ? <Notice tone="info">没有需要迁移的旧记忆。</Notice> : <form onSubmit={(event) => {
          event.preventDefault(); void action('migrate', async () => {
            const result = asRecord(await adminApi.request(pathFor(data, '/memory/migration/execute'), { method: 'POST', body: { preflight_hash: preflight.preflight_hash, confirmation } }));
            setMigration(result); setPreflight(null); setConfirmation('');
            notify(booleanValue(asRecord(result.migration).completed) ? '迁移已完成。' : '迁移已执行，仍有未解决项目，请核对报告。', booleanValue(asRecord(result.migration).completed) ? 'success' : 'warning'); onRefresh();
          });
        }}><label>确认已备份并输入 {stringValue(preflight.required_confirmation)}<input aria-label="迁移确认字符串" value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
          <button type="submit" className="danger-button" disabled={!!busy || confirmation !== preflight.required_confirmation}>执行已预检的迁移</button>
        </form>}
      </> : null}
      {migration ? <RawData data={migration} /> : null}
    </Panel>
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
