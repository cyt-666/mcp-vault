import { useMemo, useState } from 'react';

import { adminApi } from './api';
import {
  EmptyState,
  InfoGrid,
  InfoItem,
  Metric,
  Notice,
  Panel,
  StatusBadge,
  type NoticeTone,
} from './ui';
import { asRecord, numberValue, type JsonObject } from './view-model';
import './semantic-memory.css';

type Notify = (message: string, tone?: NoticeTone) => void;

type SourceBinding = {
  source_id: string;
  source_revision_id: string;
  observation_id?: string | null;
  evidence_ref_id?: string | null;
};

type CardItem = {
  kind: string;
  ordinal: number;
  content: string;
  observation_id?: string;
  evidence_ref_ids?: string[];
  conditions?: string[];
  exceptions?: string[];
  ordered_steps?: string[];
  unresolved_items?: string[];
  source_bindings?: SourceBinding[];
  support_operator?: string[];
};

type Card = {
  card_id?: string;
  composed_card_id?: string;
  title: string;
  kind: string;
  scope_ref: string;
  assertion_status: string;
  revision_number: number;
  items: CardItem[];
  source_bindings?: SourceBinding[];
};

type Evidence = {
  evidence_ref_id: string;
  source_id: string;
  source_revision_id: string;
  body_spans: Array<{ start_byte: number; end_byte: number; text: string }>;
  context_spans: Array<{ start_byte: number; end_byte: number; text: string }>;
};

type Status = {
  extraction_pending_count: number;
  organization_jobs: Array<{ job_id: string; state: string; safe_error_code?: string | null }>;
  rules_revision: number;
};

type ExplicitResult = {
  outcome: string;
  memory: {
    memory_id: string;
    ownership: string;
    revision: number;
    content: string;
    embedding_eligible: boolean;
    embedding_binding_present: boolean;
  };
};

type Props = { data: JsonObject | null; notify: Notify; onRefresh: () => void };

function cardId(card: Card): string {
  return card.card_id ?? card.composed_card_id ?? '';
}

function cardKind(card: Card): 'card' | 'composed_card' {
  return card.composed_card_id ? 'composed_card' : 'card';
}

function list<T>(value: unknown): T[] {
  return Array.isArray(value) ? value as T[] : [];
}

function idempotencyKey(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `admin-semantic-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function CardQualifiers({ item }: { item: CardItem }) {
  const conditions = item.conditions ?? [];
  const exceptions = item.exceptions ?? [];
  const steps = item.ordered_steps ?? [];
  const unresolved = item.unresolved_items ?? [];
  return (
    <div className="semantic-qualifiers">
      <p><strong>核心断言：</strong>{item.content || '（未提供）'}</p>
      {conditions.length > 0 ? <p><strong>条件：</strong>{conditions.join('；')}</p> : null}
      {exceptions.length > 0 ? <p><strong>例外：</strong>{exceptions.join('；')}</p> : null}
      {steps.length > 0 ? <p><strong>步骤：</strong>{steps.map((step, index) => `${index + 1}. ${step}`).join(' ')}</p> : null}
      {unresolved.length > 0 ? <p><strong>未决：</strong>{unresolved.join('；')}</p> : null}
    </div>
  );
}

export function SemanticMemoryPage({ data, notify, onRefresh }: Props) {
  const cards = list<Card>(data?.cards);
  const composedCards = list<Card>(data?.composed_cards);
  const status = asRecord(data?.status) as unknown as Status;
  const allCards = useMemo(() => [...cards, ...composedCards], [cards, composedCards]);
  const [selected, setSelected] = useState<Card | null>(null);
  const [evidence, setEvidence] = useState<Evidence | null>(null);
  const [task, setTask] = useState('');
  const [pack, setPack] = useState<JsonObject | null>(null);
  const [replace, setReplace] = useState('');
  const [remove, setRemove] = useState('');
  const [working, setWorking] = useState(false);
  const [explicitContent, setExplicitContent] = useState('');
  const [explicitKey, setExplicitKey] = useState(() => idempotencyKey());
  const [explicitResult, setExplicitResult] = useState<ExplicitResult | null>(null);

  const bindings = selected
    ? [...(selected.source_bindings ?? []), ...selected.items.flatMap((item) => item.source_bindings ?? [])]
    : [];

  async function showEvidence(binding: SourceBinding) {
    if (!binding.evidence_ref_id) return;
    try {
      const params = new URLSearchParams({
        evidence_ref_id: binding.evidence_ref_id,
        source_id: binding.source_id,
        source_revision_id: binding.source_revision_id,
        parent_ref: `${cardKind(selected!)}:${cardId(selected!)}`,
      });
      const result = await adminApi.request<{ evidence: Evidence }>(`/semantic/evidence?${params.toString()}`);
      setEvidence(result.evidence);
    } catch (error: unknown) {
      notify(error instanceof Error ? error.message : '证据暂时不可用。', 'danger');
    }
  }

  async function buildPack() {
    if (!task.trim()) {
      notify('请先填写当前任务或查询。', 'warning');
      return;
    }
    setWorking(true);
    try {
      const result = await adminApi.request<JsonObject>('/semantic/pack', {
        method: 'POST',
        body: { task: task.trim(), include_m1: true, include_m2: true, max_entries: 20, max_bytes: 24000, max_tokens: 6000 },
      });
      setPack(result);
    } catch (error: unknown) {
      notify(error instanceof Error ? error.message : 'Memory Pack 暂时不可用。', 'danger');
    } finally {
      setWorking(false);
    }
  }

  async function rememberExplicit() {
    if (!explicitContent.trim()) {
      notify('请先填写要保存的授权原文记忆。', 'warning');
      return;
    }
    setWorking(true);
    try {
      const result = await adminApi.request<{ explicit: ExplicitResult }>('/semantic/remember-explicit', {
        method: 'POST',
        body: { content: explicitContent, idempotency_key: explicitKey },
      });
      setExplicitResult(result.explicit);
      setExplicitContent('');
      setExplicitKey(idempotencyKey());
      notify('用户授权原文记忆已保存。');
    } catch (error: unknown) {
      notify(error instanceof Error ? error.message : '授权原文记忆保存失败，请重试。', 'danger');
    } finally {
      setWorking(false);
    }
  }

  async function mutate(mutation: 'correction' | 'forget_current') {
    if (!selected) return;
    setWorking(true);
    try {
      await adminApi.request<JsonObject>(`/semantic/${mutation === 'correction' ? 'correct' : 'forget'}`, {
        method: 'POST',
        body: {
          target_ref: `${cardKind(selected)}:${cardId(selected)}`,
          mutation,
          payload: mutation === 'correction' ? { replace: replace.trim(), remove: remove.trim() } : { reason: 'Admin semantic UI' },
          expected_parent_revision: selected.revision_number,
          expected_rules_revision: numberValue(status.rules_revision),
          idempotency_key: idempotencyKey(),
        },
      });
      notify(mutation === 'correction' ? '语义修正已提交。' : '语义记忆已隐藏；来源文件未删除。');
      setEvidence(null);
      onRefresh();
    } catch (error: unknown) {
      notify(error instanceof Error ? error.message : '语义操作失败，请刷新后重试。', 'danger');
    } finally {
      setWorking(false);
    }
  }

  return (
    <div className="page-stack semantic-memory-page">
      <section className="hero-card">
        <div>
          <StatusBadge tone="success">来源受保护</StatusBadge>
          <h2>语义记忆</h2>
          <p>查看当前已验证的 M1 卡片与 M2 组合卡；这里只展示结构化语义，不展示普通笔记正文。</p>
        </div>
      </section>

      <Panel title="用户授权原文记忆" eyebrow="独立 explicit memory" description="按用户授权原样保存；它不是语义卡、evidence 或 Memory Pack，也不会修改普通笔记。">
        <div className="semantic-explicit-form">
          <textarea value={explicitContent} onChange={(event) => setExplicitContent(event.target.value)} placeholder="填写要长期保留的用户授权原文…" aria-label="授权原文记忆" rows={4} />
          <div className="button-row">
            <button className="primary-button" type="button" disabled={working || !explicitContent.trim()} onClick={() => void rememberExplicit()}>{working ? '保存中…' : '保存原文记忆'}</button>
            <span className="muted-text">重试会复用当前幂等键；成功后下一次提交自动生成新键。</span>
          </div>
        </div>
        {explicitResult ? <div className="semantic-explicit-result" role="status">
          <StatusBadge tone="success">已保存 · {explicitResult.outcome}</StatusBadge>
          <InfoGrid>
            <InfoItem label="ownership" value={explicitResult.memory.ownership} />
            <InfoItem label="memory ID" value={explicitResult.memory.memory_id} mono />
            <InfoItem label="revision" value={explicitResult.memory.revision} mono />
            <InfoItem label="embedding eligible" value={explicitResult.memory.embedding_eligible ? '是' : '否'} />
            <InfoItem label="binding state" value={explicitResult.memory.embedding_binding_present ? '已配置' : '未配置（不外发）'} />
          </InfoGrid>
        </div> : null}
      </Panel>

      <section className="metrics-grid semantic-metrics">
        <Metric label="M1 卡片" value={cards.length} detail="当前可读" />
        <Metric label="M2 组合卡" value={composedCards.length} detail="AND/OR 支持已验证" />
        <Metric label="待处理提取" value={numberValue(status.extraction_pending_count)} detail="只显示安全计数" />
        <Metric label="规则修订" value={numberValue(status.rules_revision)} detail="mutation fence" />
      </section>

      <Panel title="Memory Pack" eyebrow="当前任务上下文" description="无答案时明确显示缺口，不会用普通笔记填充语义结果。">
        <div className="semantic-pack-form">
          <input value={task} onChange={(event) => setTask(event.target.value)} placeholder="例如：当前 WebDAV 冲突处理有哪些约束？" aria-label="语义任务" />
          <button className="primary-button" type="button" disabled={working} onClick={() => void buildPack()}>{working ? '处理中…' : '构建 Pack'}</button>
        </div>
        {pack ? <div className="semantic-pack-result">
          <div className="button-row"><StatusBadge tone={list(pack.evidence_gaps).length > 0 ? 'warning' : 'success'}>{list(pack.evidence_gaps).length > 0 ? '有证据缺口' : '有匹配结果'}</StatusBadge><span>estimated tokens: {numberValue(pack.estimated_tokens)}</span></div>
          {list(pack.current_context).length === 0 && list(pack.relevant_experiences).length === 0 ? <Notice tone="warning">没有足够的当前语义答案；请查看下方 evidence gaps 与 diagnostics。</Notice> : null}
          {list(pack.evidence_gaps).map((gap) => <Notice key={JSON.stringify(gap)} tone="warning">缺口：{JSON.stringify(gap)}</Notice>)}
          {list(pack.diagnostics).map((diagnostic) => <p key={String(diagnostic)} className="muted-text">诊断：{String(diagnostic)}</p>)}
        </div> : null}
      </Panel>

      <Panel title="当前语义卡片" eyebrow="M1 + M2" description="选择一张卡片查看限定条件、证据引用和安全 mutation。">
        {allCards.length === 0 ? <EmptyState title="当前没有可读语义卡片" detail="来源可能仍在提取、被 suppression 隐藏，或需要重新建立证据。" /> : (
          <div className="semantic-card-grid">
            {allCards.map((card) => {
              const id = cardId(card);
              const kind = cardKind(card);
              return <button key={`${kind}:${id}`} type="button" className={`semantic-card${selected && cardId(selected) === id ? ' semantic-card--selected' : ''}`} onClick={() => { setSelected(card); setEvidence(null); }}>
                <span className="eyebrow">{kind === 'composed_card' ? 'M2 组合卡' : 'M1 卡片'}</span>
                <strong>{card.title}</strong>
                <span>{card.kind} · {card.scope_ref}</span>
                <span>{card.assertion_status} · revision {card.revision_number}</span>
              </button>;
            })}
          </div>
        )}
      </Panel>

      {selected ? <Panel title={selected.title} eyebrow={cardKind(selected) === 'composed_card' ? 'M2 组合卡' : 'M1 卡片'}>
        <InfoGrid>
          <InfoItem label="kind" value={selected.kind} />
          <InfoItem label="scope" value={selected.scope_ref} />
          <InfoItem label="status" value={selected.assertion_status} />
          <InfoItem label="parent revision" value={selected.revision_number} mono />
        </InfoGrid>
        {selected.items.map((item) => <div className="semantic-item" key={`${item.observation_id ?? item.ordinal}:${item.ordinal}`}><CardQualifiers item={item} />{item.support_operator ? <p className="muted-text">支持关系：{item.support_operator.join(' / ')}</p> : null}</div>)}
        {bindings.length > 0 ? <div className="semantic-evidence-list"><strong>证据引用</strong>{bindings.map((binding) => <button className="secondary-button" type="button" key={`${binding.source_id}:${binding.evidence_ref_id}`} onClick={() => void showEvidence(binding)}>展开 evidence {binding.evidence_ref_id}</button>)}</div> : <p className="muted-text">当前卡片没有可展开的 evidence parent。</p>}
        {evidence ? <div className="semantic-evidence-detail"><h3>Evidence spans</h3><p className="muted-text">只展示被验证的 body/context span，不读取整篇普通笔记。</p>{[...evidence.body_spans.map((span) => ({ ...span, role: 'body' })), ...evidence.context_spans.map((span) => ({ ...span, role: 'context' }))].map((span) => <div className="span-row" key={`${span.role}:${span.start_byte}:${span.end_byte}`}><StatusBadge>{span.role}</StatusBadge><code>{span.start_byte}..{span.end_byte}</code><span>{span.text}</span></div>)}</div> : null}
        <div className="semantic-mutation"><h3>安全操作</h3><div className="semantic-pack-form"><input value={remove} onChange={(event) => setRemove(event.target.value)} placeholder="修正：旧断言（仅 correction）" aria-label="旧断言" /><input value={replace} onChange={(event) => setReplace(event.target.value)} placeholder="修正：新断言（仅 correction）" aria-label="新断言" /><button className="primary-button" type="button" disabled={working || !remove.trim() || !replace.trim()} onClick={() => void mutate('correction')}>Correct</button><button className="danger-button" type="button" disabled={working} onClick={() => void mutate('forget_current')}>Forget</button></div><p className="muted-text">请求由服务端补充 actor、Vault scope、当前 parent/rules revision 与幂等键；不会删除来源文件。</p></div>
      </Panel> : null}
    </div>
  );
}
