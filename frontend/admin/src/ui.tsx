import { useState } from 'react';
import type { ReactNode } from 'react';

import type { JsonObject } from './view-model';

export type NoticeTone = 'success' | 'warning' | 'danger' | 'info';

export function Panel({
  title,
  eyebrow,
  description,
  actions,
  children,
  className = '',
}: {
  title: string;
  eyebrow?: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <article className={`panel ${className}`.trim()}>
      <div className="panel-heading">
        <div>
          {eyebrow ? <p className="eyebrow">{eyebrow}</p> : null}
          <h2>{title}</h2>
          {description ? <p className="panel-description">{description}</p> : null}
        </div>
        {actions ? <div className="panel-actions">{actions}</div> : null}
      </div>
      {children}
    </article>
  );
}

export function Metric({ label, value, detail }: { label: string; value: string | number; detail: string }) {
  return (
    <article className="metric-card">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </article>
  );
}

export function StatusBadge({
  children,
  tone = 'neutral',
}: {
  children: ReactNode;
  tone?: 'success' | 'warning' | 'danger' | 'neutral';
}) {
  return <span className={`status-badge status-badge--${tone}`}>{children}</span>;
}

export function InfoGrid({ children }: { children: ReactNode }) {
  return <dl className="info-grid">{children}</dl>;
}

export function InfoItem({ label, value, mono = false }: { label: string; value: ReactNode; mono?: boolean }) {
  return (
    <div className="info-item">
      <dt>{label}</dt>
      <dd className={mono ? 'mono-value' : undefined}>{value}</dd>
    </div>
  );
}

export function Notice({ children, tone = 'info' }: { children: ReactNode; tone?: NoticeTone }) {
  return (
    <div className={`notice notice--${tone}`} role={tone === 'danger' ? 'alert' : 'status'}>
      {children}
    </div>
  );
}

export function EmptyState({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="empty-state">
      <strong>{title}</strong>
      <p>{detail}</p>
    </div>
  );
}

export function InlineAlert({ message, onDismiss }: { message: string; onDismiss?: () => void }) {
  return (
    <div className="inline-alert" role="alert">
      <span>{message}</span>
      {onDismiss ? (
        <button type="button" onClick={onDismiss} aria-label="关闭错误提示">
          关闭
        </button>
      ) : null}
    </div>
  );
}

export function LoadingBar() {
  return (
    <div className="loading-bar" role="status">
      <span aria-hidden="true" />正在读取最新状态…
    </div>
  );
}

export function PasswordPolicyHelp({ id }: { id: string }) {
  return (
    <small className="password-policy-help" id={id}>
      <span><strong>密码要求：</strong>最低 12 字节，即纯英文至少 12 个字符、常用汉字至少 4 个；中文密码仍建议至少 8 个汉字。</span>
      <span>无需强制包含大小写、数字或符号；不能直接使用 password、password123、changeme、admin、admin123 或 letmein。</span>
    </small>
  );
}

export function RawData({ data }: { data: JsonObject | null }) {
  if (!data) return null;
  return (
    <details className="raw-details">
      <summary>高级：查看原始响应</summary>
      <p>用于排障的原始 API 响应，可能包含本页已展示的路径、内容和标识；不会返回已保存的完整密钥。</p>
      <pre className="data-inspector">{JSON.stringify(data, null, 2)}</pre>
    </details>
  );
}

export function CopyField({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  }

  return (
    <div className="copy-field">
      <div>
        <span>{label}</span>
        <code>{value}</code>
      </div>
      <button className="secondary-button" type="button" onClick={() => void copy()}>
        {copied ? '已复制' : '复制'}
      </button>
    </div>
  );
}

export function SecretReveal({ secret, onDismiss }: { secret: string; onDismiss: () => void }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  return (
    <section className="secret-reveal" aria-live="assertive">
      <div>
        <p className="eyebrow">仅显示一次</p>
        <h3>现在复制并妥善保存</h3>
        <code>{secret}</code>
        <small>关闭后无法再次从服务端查看完整内容。</small>
      </div>
      <div className="button-row">
        <button className="primary-button" type="button" onClick={() => void copy()}>
          {copied ? '已复制' : '复制密钥'}
        </button>
        <button className="secondary-button" type="button" onClick={onDismiss}>
          我已保存
        </button>
      </div>
    </section>
  );
}
