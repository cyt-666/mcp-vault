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
  maintenance: '服务正在维护，当前操作暂时不可用。',
  not_found: '没有找到对应记录。',
  backup_unavailable: '备份服务暂时不可用。',
  provider_unavailable: 'AI 服务暂时不可用。',
  capability_unavailable: '所选模型不支持这项用途。',
  model_exists: '这个模型 ID 已经登记过了。',
  memory_extraction_not_ready: '记忆提取尚未就绪，请先启用策略、允许 AI 调用并绑定记忆提取模型。',
  memory_extraction_model_unbound: '尚未给记忆提取绑定模型。',
  memory_extraction_model_missing: '记忆提取绑定的模型不存在，请重新选择。',
  memory_pipeline_reset_pending: '新版记忆系统正在清理旧数据，请等待自动重置完成。',
  memory_pipeline_regeneration_pending: '新版记忆系统正在创建必须的全量重新提取任务，请稍候。',
  memory_unavailable: '记忆服务暂时不可用。',
  index_unavailable: '知识索引暂时不可用。',
};

const jobErrorMessages: Record<string, string> = {
  memory_pipeline_reset_waiting_for_jobs: '正在等待旧记忆任务安全停止',
  memory_pipeline_reset_quiesce_failed: '暂时无法停止旧记忆任务，将自动重试',
  memory_pipeline_reset_retryable: '记忆系统重置暂时失败，将自动重试',
  memory_pipeline_reset_failed: '记忆系统重置失败，请检查服务日志',
  memory_pipeline_regeneration_admission_failed: '旧数据已清理，但暂时无法创建全量重新提取任务',
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
  memory_phase1_output_invalid: 'AI 返回的阶段一结果无法解析',
  memory_phase1_no_output_inconsistent: 'AI 同时返回了空记忆和非空辅助字段',
  memory_phase1_output_too_large: 'AI 返回的原始记忆或来源摘要超过大小上限',
  memory_phase1_evidence_missing: 'AI 返回了原始记忆，但没有提供支持证据',
  memory_phase1_evidence_too_many: 'AI 返回的支持证据超过每篇笔记上限',
  memory_phase1_slug_invalid: 'AI 返回的来源标识格式不正确',
  memory_phase1_evidence_anchor_invalid: 'AI 返回的证据行号超出笔记范围',
  memory_phase1_evidence_too_large: 'AI 选择的单段证据范围超过 16 KiB 上限',
  memory_phase2_output_invalid: 'AI 返回的阶段二结果无法解析为整理方案',
  memory_phase2_output_bounds: 'AI 返回的阶段二摘要、操作或丢弃列表超过安全上限',
  memory_phase2_memory_id_missing: 'AI 的更新或归档操作没有引用现有记忆 ID',
  memory_phase2_memory_id_invalid: 'AI 的更新或归档操作返回了无效的记忆 ID',
  memory_phase2_memory_index_missing: 'AI 的更新或归档操作没有引用当前记忆序号',
  memory_phase2_memory_index_invalid: 'AI 引用了当前整理快照中不存在的记忆序号',
  memory_phase2_memory_unknown: 'AI 的更新或归档操作引用了不存在的当前记忆',
  memory_phase2_content_missing: 'AI 的新建或更新操作缺少记忆内容',
  memory_phase2_content_invalid: 'AI 返回的长期记忆内容为空、过长或包含无效字符',
  memory_phase2_memory_type_missing: 'AI 的新建或更新操作缺少记忆类型',
  memory_phase2_metadata_missing: 'AI 的新建或更新操作缺少类型或原始记忆引用',
  memory_phase2_stage1_missing: 'AI 的新建或更新操作没有引用阶段一原始记忆',
  memory_phase2_stage1_id_invalid: 'AI 返回了格式无效的阶段一原始记忆 ID',
  memory_phase2_stage1_unknown: 'AI 引用了当前整理快照中不存在的阶段一原始记忆',
  memory_phase2_stage1_invalid: 'AI 重复引用了原始记忆，或引用的原始记忆当前不可用',
  memory_phase2_input_index_invalid: 'AI 引用了不存在、重复或当前不可用的原始记忆序号',
  memory_phase2_discard_unknown: 'AI 要求丢弃的原始记忆不属于本次待处理输入',
  memory_phase2_discard_index_invalid: 'AI 要求丢弃的原始记忆序号不属于本次待处理输入',
  memory_phase2_discard_duplicate: 'AI 重复列出了要丢弃的同一条原始记忆',
  memory_phase2_input_undispositioned: 'AI 既未使用也未明确丢弃某条待处理原始记忆',
  memory_phase2_input_status_invalid: '待整理原始记忆具有无法处理的状态',
  memory_phase2_disposition_unknown: '已准备的整理方案包含未知原始记忆',
  memory_phase2_disposition_invalid: '已准备的整理方案重复处理同一原始记忆或包含无效原因',
  memory_phase2_disposition_status_invalid: '整理方案对原始记忆的处理方式与当前状态冲突',
  memory_phase2_disposition_conflict: 'AI 同时使用并丢弃了同一条原始记忆',
  memory_phase2_action_invalid: 'AI 返回了无效或自相矛盾的阶段二操作',
  memory_phase2_action_duplicate: 'AI 在同一整理方案中多次修改同一条记忆',
  memory_phase2_evidence_missing: '阶段二写入缺少阶段一已经验证的来源证据',
  memory_phase2_evidence_invalid: '阶段二方案引用的来源证据与阶段一记录不一致',
  memory_phase2_supersession_id_invalid: 'AI 返回了格式无效的被替代记忆 ID',
  memory_phase2_supersession_index_invalid: 'AI 引用了不存在的被替代记忆序号',
  memory_phase2_supersession_invalid: 'AI 返回了无效、重复或自相矛盾的记忆替代关系',
  memory_phase2_prepared_invalid: '已准备的阶段二方案无法安全恢复，请检查任务与数据库状态',
  memory_consolidation_waiting_for_phase1: '阶段二正在等待阶段一完成，不会消耗重试次数',
  memory_consolidation_phase1_state_failed: '暂时无法读取阶段一任务状态，将自动重试',
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
  provider_schema_invalid: 'AI 返回了 JSON，但阶段输出结构不符合要求',
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
  candidate: '旧版候选（待迁移）',
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

export function jobErrorLabel(value: unknown): string {
  const code = stringValue(value, 'unknown');
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
