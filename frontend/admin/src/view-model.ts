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
    description: '查看自动生成的长期记忆、来源状态和异常处理结果。',
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
  vault_selection_required: '请选择要管理的 Vault。',
  vault_already_exists: '这个 Vault 链接标识已经存在。',
  vault_root_unavailable: '服务托管目录已存在内容或无法安全使用。',
  vault_disabled: '请先启用该 Vault。',
  vault_already_initialized: '该 Vault 已经完成初始化。',
  maintenance: '服务正在维护，当前操作暂时不可用。',
  not_found: '没有找到对应记录。',
  backup_unavailable: '备份服务暂时不可用。',
  provider_unavailable: 'AI 服务暂时不可用。',
  capability_unavailable: '所选模型不支持这项用途。',
  model_exists: '这个模型 ID 已经登记过了。',
  memory_extraction_not_ready: '记忆提取尚未就绪，请先启用策略、允许 AI 调用并绑定记忆提取模型。',
  memory_extraction_model_unbound: '尚未给记忆提取绑定模型。',
  memory_extraction_model_missing: '记忆提取绑定的模型不存在，请重新选择。',
  memory_migration_preflight_stale: '迁移预检结果已过期；请重新预检并确认。',
  memory_unavailable: '记忆服务暂时不可用。',
  index_unavailable: '知识索引暂时不可用。',
};

const jobErrorMessages: Record<string, string> = {
  memory_source_reconcile_vault_lookup_failed: '暂时无法读取待协调的 Vault，将自动重试',
  memory_source_reconcile_vault_missing: '待协调的 Vault 已不存在',
  memory_source_reconcile_file_missing: '来源文件记录已不存在',
  memory_source_reconcile_context_invalid: '无法建立安全的 Vault 上下文',
  memory_source_reconcile_core_unavailable: '暂时无法访问来源文件，将自动重试',
  memory_source_reconcile_retryable: '记忆来源协调暂时失败，将自动重试',
  memory_source_reconcile_failed: '记忆来源协调失败，请检查服务日志',
  memory_source_reconcile_extract_admission_failed: '来源已协调，但暂时无法提交重新提取任务，将自动重试',
  memory_source_reconcile_progress_failed: '来源协调结果暂时无法保存，将自动重试',
  memory_extract_vault_lookup_failed: '暂时无法读取待处理的 Vault，将自动重试',
  memory_extract_vault_missing: '待处理的 Vault 已不存在',
  memory_extract_context_invalid: '无法建立安全的 Vault 上下文',
  memory_extract_core_unavailable: '暂时无法访问 Vault 文件，将自动重试',
  memory_extract_source_list_failed: '暂时无法枚举待处理笔记，将自动重试',
  memory_extract_retryable: '记忆提取暂时失败，将自动重试',
  memory_extract_input_invalid: '笔记内容不符合记忆提取要求',
  memory_extract_not_found: '待提取的笔记已不存在',
  memory_extract_path_invalid: '任务中的笔记路径无效',
  memory_extract_lease_missing: '记忆提取任务没有有效执行租约',
  memory_extract_progress_failed: '记忆提取进度暂时无法保存，将自动重试',
  memory_extract_progress_finalize_failed: '模型调用可能已经完成，但任务进度无法落盘；为避免重复计费，任务已停止，请检查数据库状态后再手动重试',
  memory_extract_output_failure_limit: '连续 3 次模型输出都不符合契约，已暂停后续调用以避免持续无效计费；修正模型兼容设置后可从当前进度重试',
  memory_source_not_found: '笔记在处理前已被删除或移动',
  memory_source_read_failed: '无法读取笔记内容',
  memory_source_too_large: '笔记达到或超过 512 KiB 的单篇提取上限',
  memory_source_not_utf8: '笔记不是 UTF-8 文本',
  memory_source_hash_missing: '来源文件缺少可验证的内容哈希',
  memory_set_canonical_read_failed: '当前记忆集合的规范 Markdown 无法读取',
  memory_set_output_invalid: 'AI 返回的当前记忆集合不是有效对象',
  memory_set_too_many_items: 'AI 返回的当前记忆条目超过每篇笔记上限',
  memory_set_item_invalid: 'AI 返回了空白、过长或含非法字符的记忆内容',
  memory_set_snapshot_invalid: '已保存的当前集合快照无法安全恢复',
  memory_set_snapshot_hash_mismatch: '当前集合快照与来源内容不一致，必须重新提取',
  memory_extraction_disabled: '记忆自动提取已停用',
  memory_extraction_model_unbound: '尚未绑定记忆提取模型',
  memory_extraction_model_missing: '绑定的记忆提取模型不存在',
  provider_connect_failed: '无法连接 AI 服务，将自动重试',
  provider_dns_failed: '无法解析 AI 服务地址，将自动重试',
  provider_request_failed: 'AI 请求发送或等待响应时失败；请求是否已被远端处理无法确认，请检查网络后手动重试',
  provider_timeout: 'AI 服务响应超时，将自动重试',
  provider_response_timeout: 'AI 服务已接受请求，但响应正文未在记忆提取时限内读取完成；为避免重复计费，不会自动重试',
  provider_response_incomplete: 'AI 服务已接受请求，但响应正文中途断开或不完整；为避免重复计费，不会自动重试',
  provider_response_read_failed: 'AI 服务已接受请求，但响应正文读取失败；为避免重复计费，不会自动重试',
  provider_response_too_large: 'AI 服务响应超过配置的大小上限',
  provider_http_error: 'AI 服务返回了不可重试的 HTTP 错误',
  embedding_dimension_mismatch: '向量维度与登记模型不一致，请检查模型默认维度',
  provider_rate_limited: 'AI 服务触发限流，将自动重试',
  provider_server_error: 'AI 服务端暂时异常，将自动重试',
  provider_auth_failed: 'AI 服务认证失败，请检查密钥',
  provider_endpoint_denied: 'AI 服务地址被安全策略拒绝',
  provider_capability_unavailable: '所选模型不支持这项用途',
  provider_response_content_type_invalid: 'AI 服务返回的正文不是 JSON；请检查 API 地址是否指向兼容接口，而不是网页或代理错误页',
  provider_response_json_invalid: 'AI 服务返回的 HTTP 正文不是有效 JSON；请检查兼容接口或反向代理',
  provider_final_content_missing: 'AI 服务返回了成功响应，但没有最终文本内容；请检查模型兼容模式和模型平台日志',
  provider_structured_json_invalid: 'AI 服务返回了文本，但不是完整的 JSON；请检查模型兼容模式或输出是否被截断',
  provider_output_truncated: 'AI 输出达到 Token 上限，结构化 JSON 未完成；任务不会自动重复计费，请调整模型输出上限后手动重试',
  provider_output_filtered: 'AI 输出被模型平台的内容策略拦截；任务不会自动重试',
  provider_output_repetition_truncated: 'AI 输出因重复内容被模型平台截断；任务不会自动重试',
  provider_response_invalid: 'AI 服务返回了无法识别的响应',
  provider_schema_invalid: 'AI 返回了 JSON，但当前记忆集合结构不符合要求',
};

const statusLabels: Record<string, string> = {
  active: '正常',
  initializing: '初始化中',
  ready: '就绪',
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

export function jobErrorLabel(value: unknown): string {
  const code = stringValue(value, 'unknown');
  if ([
    'memory_phase1_',
    'memory_phase2_',
    'memory_consolidation_',
    'memory_retrieval_',
    'memory_pipeline_',
    'memory_source_audit_',
    'memory_source_repair_',
  ].some((prefix) => code.startsWith(prefix))) {
    return '旧版记忆任务已退役；当前服务不会重新执行该流程';
  }
  return jobErrorMessages[code] ?? errorMessages[code] ?? code;
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
  if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
  const amount = value;
  const percentage = amount <= 1 ? amount * 100 : amount;
  return `${Math.max(0, Math.min(100, percentage)).toFixed(0)}%`;
}

export function truncateId(value: unknown): string {
  const id = stringValue(value);
  return id.length > 18 ? `${id.slice(0, 8)}…${id.slice(-6)}` : id;
}
