import { AdminApiError } from './api';

export type Page =
  | 'dashboard'
  | 'vault'
  | 'webdav'
  | 'mcp'
  | 'providers'
  | 'index'
  | 'memory'
  | 'jobs'
  | 'audit'
  | 'backup'
  | 'system';

export type JsonObject = Record<string, unknown>;

export type PageMeta = {
  label: string;
  shortLabel: string;
  description: string;
  icon: string;
};

export const pageMeta: Record<Page, PageMeta> = {
  dashboard: {
    label: '总览',
    shortLabel: '总览',
    description: '查看 Vault、同步、记忆和后台任务的关键状态。',
    icon: '总',
  },
  vault: {
    label: 'Vault 设置',
    shortLabel: 'Vault',
    description: '管理当前知识库的名称、状态和重新扫描。',
    icon: '库',
  },
  webdav: {
    label: 'Obsidian 同步',
    shortLabel: 'WebDAV',
    description: '为 Obsidian 设备创建独立、可撤销的 WebDAV 凭据。',
    icon: '同',
  },
  mcp: {
    label: 'Agent 接入',
    shortLabel: 'MCP',
    description: '查看 MCP 地址，并管理 PAT 与高级 OAuth 授权。',
    icon: 'M',
  },
  providers: {
    label: 'AI 服务',
    shortLabel: 'AI 服务',
    description: '配置模型提供商和当前 Vault 的数据发送策略。',
    icon: 'AI',
  },
  index: {
    label: '知识索引',
    shortLabel: '索引',
    description: '查看全文索引覆盖率，并在需要时安全重建。',
    icon: '索',
  },
  memory: {
    label: '长期记忆',
    shortLabel: '记忆',
    description: '检查已生效记忆、来源状态和待审核候选。',
    icon: '忆',
  },
  jobs: {
    label: '后台任务',
    shortLabel: '任务',
    description: '查看持久化任务、重试进度和失败原因。',
    icon: '任',
  },
  backup: {
    label: '备份与恢复',
    shortLabel: '备份',
    description: '创建和验证备份；恢复操作按需展开。',
    icon: '备',
  },
  audit: {
    label: '审计日志',
    shortLabel: '审计',
    description: '查看经过脱敏的管理操作和安全事件。',
    icon: '审',
  },
  system: {
    label: '系统信息',
    shortLabel: '系统',
    description: '查看监听器、数据库、迁移和运行状态。',
    icon: '系',
  },
};

export const navigationGroups: Array<{ label: string; pages: Page[] }> = [
  { label: '常用', pages: ['dashboard', 'vault'] },
  { label: '连接', pages: ['webdav', 'mcp'] },
  { label: '智能', pages: ['providers', 'index', 'memory'] },
  { label: '运维', pages: ['jobs', 'backup', 'audit', 'system'] },
];

const errorMessages: Record<string, string> = {
  admin_session_required: '请先登录管理端。',
  admin_session_invalid: '登录状态无效，请重新登录。',
  admin_session_expired: '登录已过期，请重新登录。',
  authentication_failed: '用户名或密码不正确。',
  setup_unavailable: '首次初始化已经完成，不能再次创建管理员。',
  origin_rejected: '当前访问地址不在允许的管理端来源列表中。',
  csrf_rejected: '安全校验失败，请刷新页面后重试。',
  rate_limited: '尝试次数过多，请稍后再试。',
  password_policy: '密码不符合要求：最低 12 字节，即纯英文至少 12 个字符、常用汉字至少 4 个；不能直接使用 password、password123、changeme、admin、admin123 或 letmein；无需强制组合大小写、数字或符号。',
  validation_failed: '填写内容有误，请检查后重试。',
  revision_conflict: '配置已被其他操作更新，请刷新后再保存。',
  state_unavailable: '运行状态暂时不可用，请稍后重试。',
  maintenance: '服务正在维护，当前操作暂时不可用。',
  not_found: '没有找到对应记录。',
  backup_unavailable: '备份服务暂时不可用。',
  provider_unavailable: 'AI 服务暂时不可用。',
  memory_unavailable: '记忆服务暂时不可用。',
  index_unavailable: '知识索引暂时不可用。',
};

const statusLabels: Record<string, string> = {
  active: '正常',
  enabled: '已启用',
  disabled: '已停用',
  maintenance: '维护中',
  error: '异常',
  healthy: '正常',
  degraded: '降级',
  unknown: '未知',
  queued: '等待中',
  running: '执行中',
  retry_wait: '等待重试',
  completed: '已完成',
  failed: '失败',
  cancelled: '已取消',
  verified: '已验证',
  pending: '等待中',
  candidate: '待审核',
  stale: '来源已失效',
  superseded: '已被替代',
  archived: '已归档',
  rejected: '已拒绝',
  quarantined: '已隔离',
  disabled_provider: '已禁用',
  local_only: '仅本地服务',
  remote_allowed: '允许远程 HTTPS',
};

const memoryTypeLabels: Record<string, string> = {
  identity: '身份',
  preference: '偏好',
  decision: '决策',
  constraint: '约束',
  fact: '事实',
  project: '项目',
  progress: '进展',
  event: '事件',
  relationship: '关系',
  procedure: '流程',
};

export function formatRequestError(error: unknown): string {
  if (error instanceof AdminApiError) {
    return errorMessages[error.code] ?? `请求失败（${error.code}），请稍后重试。`;
  }
  return '请求失败，请检查服务状态和网络连接。';
}

export function asRecord(value: unknown): JsonObject {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as JsonObject) : {};
}

export function arrayRecords(value: unknown): JsonObject[] {
  return Array.isArray(value) ? value.map(asRecord) : [];
}

export function stringValue(value: unknown, fallback = '—'): string {
  return typeof value === 'string' && value.length > 0 ? value : fallback;
}

export function numberValue(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

export function booleanValue(value: unknown): boolean {
  return value === true || value === 1;
}

export function statusLabel(value: unknown): string {
  const status = stringValue(value, 'unknown');
  return statusLabels[status] ?? status;
}

export function statusTone(value: unknown): 'success' | 'warning' | 'danger' | 'neutral' {
  const status = stringValue(value, 'unknown');
  if (['active', 'enabled', 'healthy', 'completed', 'verified'].includes(status)) return 'success';
  if (['queued', 'running', 'retry_wait', 'pending', 'candidate', 'maintenance', 'degraded'].includes(status)) return 'warning';
  if (['failed', 'error', 'quarantined', 'rejected'].includes(status)) return 'danger';
  return 'neutral';
}

export function memoryTypeLabel(value: unknown): string {
  const memoryType = stringValue(value, 'unknown');
  return memoryTypeLabels[memoryType] ?? memoryType;
}

export function formatTime(value: unknown): string {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return '—';
  const milliseconds = value < 1_000_000_000_000 ? value * 1000 : value;
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(milliseconds));
}

export function formatBytes(value: unknown): string {
  const bytes = numberValue(value, -1);
  if (bytes < 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let amount = bytes / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && amount >= 1024; index += 1) {
    amount /= 1024;
    unit = units[index];
  }
  return `${amount.toFixed(amount >= 10 ? 1 : 2)} ${unit}`;
}

export function formatPercent(value: unknown): string {
  const amount = numberValue(value, 0);
  const percentage = amount <= 1 ? amount * 100 : amount;
  return `${Math.max(0, Math.min(100, percentage)).toFixed(0)}%`;
}

export function truncateId(value: unknown): string {
  const id = stringValue(value);
  return id.length > 18 ? `${id.slice(0, 8)}…${id.slice(-6)}` : id;
}
