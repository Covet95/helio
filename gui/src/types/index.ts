/** Codex /model catalog 条目（精简） */
export interface CodexCatalogModel {
  slug: string;
  display_name?: string;
  context_window?: number;
  reasoning_levels?: string[];
  supports_reasoning?: boolean;
  supports_images?: boolean;
  supports_tool_calls?: boolean;
  supports_web_search?: boolean;
}

export interface ApiProfile {
  id?: number;
  name: string;
  provider: string;
  api_url: string;
  /** 活跃 key（与 adapters switch 对齐；等于 api_keys 中 is_active 的那把） */
  api_key: string;
  /** 多 key 池；空/缺省时仅用 api_key */
  api_keys?: ApiKeyEntry[];
  model_mapping?: Record<string, string>;
  /** 默认模型 */
  model?: string;
  /** OpenCode 专用：provider 下挂载的模型列表（多选） */
  models?: string[];
  /** Codex：写入 model_catalog.json 的模型目录（/model 列表） */
  catalog_models?: CodexCatalogModel[];
  /** 推理强度 minimal/low/medium/high/xhigh */
  reasoning_effort?: string;
  /** 1M 上下文 */
  context_1m?: boolean;
  /** OpenClaw: models[].maxTokens（仅 OpenClaw 使用，不与 Hermes 共用语义） */
  max_tokens?: number;
  /** Codex legacy import compatibility; generated config always uses Responses. */
  wire_api?: string;
  /** Codex provider-scoped API key environment variable. */
  env_key?: string;
  /** Codex legacy import compatibility. */
  requires_openai_auth?: boolean;
  /** Codex legacy import compatibility. */
  experimental_bearer_token?: string;
  /** Codex 顶层 service_tier（如 fast） */
  service_tier?: string;
  /** Custom Codex provider declares standalone web-search support. */
  supports_standalone_web_search?: boolean;
  /** Built-in Amazon Bedrock profile override. */
  aws_profile?: string;
  /** Built-in Amazon Bedrock profile override. */
  aws_region?: string;
  /**
   * 协议模式。Hermes → model.api_mode / custom_providers[].api_mode；
   * OpenClaw → models.providers.<id>.api。各工具独立解释，不共用适配逻辑。
   */
  api_mode?: string;
  /** 归属工具；undefined = 通用（所有工具下都显示）*/
  target_app?: TargetApp;
  created_at?: number;
  updated_at?: number;
}

export interface FetchedModel {
  id: string;
  owned_by?: string;
}

export interface ModelTestResult {
  model: string;
  endpoint: string;
  /** chat_completions | responses | anthropic_messages | gemini */
  protocol?: string;
  key_label?: string;
}

/** 同一 profile 下的一把 API Key */
export interface ApiKeyEntry {
  id: string;
  label: string;
  key: string;
  is_active: boolean;
  last_probe_ok?: boolean | null;
  last_probed_at?: number | null;
  created_at?: number;
}

export type TargetApp = 'claude-code' | 'codex' | 'pi' | 'opencode' | 'hermes' | 'openclaw';

/// 已注册工具的元数据，用于动态生成 UI
export interface ToolInfo {
  id: TargetApp;
  displayName: string;
  /** short mark shown in the icon tile (terminal-style) */
  short: string;
  /** brand accent color (tailwind text/bg via arbitrary value) */
  color: string;
  /** config format hint */
  format: string;
}

export const SUPPORTED_TOOLS: ToolInfo[] = [
  { id: 'claude-code', displayName: 'Claude Code', short: 'CC', color: '#8A5A44', format: 'JSON' },
  { id: 'codex', displayName: 'Codex', short: 'CX', color: '#10B981', format: 'TOML' },
  { id: 'pi', displayName: 'Pi', short: 'PI', color: '#4F8DF6', format: 'JSON' },
  { id: 'opencode', displayName: 'OpenCode', short: 'OC', color: '#4B5563', format: 'JSON' },
  { id: 'hermes', displayName: 'Hermes', short: 'HM', color: '#7C3AED', format: 'YAML' },
  { id: 'openclaw', displayName: 'OpenClaw', short: 'OCW', color: '#0EA5E9', format: 'JSON' },
];

export function toolById(id: TargetApp | string): ToolInfo | undefined {
  return SUPPORTED_TOOLS.find((t) => t.id === id);
}

export interface TargetStatus {
  profile?: ApiProfile;
  connected: boolean;
  latency?: number;
  probe_ok?: boolean | null;
  probe_error?: string | null;
  last_probed_at?: number | null;
  probe_protocol?: string | null;
  latency_ms?: number | null;
}

/** 对齐 CC Switch HealthStatus：operational | degraded | failed */
export type ReachabilityStatus = 'operational' | 'degraded' | 'failed';

export interface ToolProbeResult {
  target_app: string;
  configured: boolean;
  /** 任意 HTTP 响应 = 可达（与 CC Switch stream_check 一致） */
  ok: boolean;
  /** operational | degraded | failed */
  status?: ReachabilityStatus | string;
  profile_name?: string;
  error?: string;
  /** 可达性探测恒为 "reachability" */
  protocol?: string;
  endpoint?: string;
  latency_ms?: number;
  http_status?: number;
  probed_at: number;
}

export interface DatabaseInfo {
  size: number;
  profile_count: number;
  path: string;
}

export interface StatusInfo {
  claude_code?: TargetStatus;
  codex?: TargetStatus;
  pi?: TargetStatus;
  opencode?: TargetStatus;
  hermes?: TargetStatus;
  openclaw?: TargetStatus;
  database: DatabaseInfo;
}

export interface SessionMeta {
  id: string;
  tool: string;
  cwd: string;
  title: string | null;
  started_at: number;
  modified_at: number;
  size_bytes: number;
  message_count: number;
  parseable: boolean;
}

export interface PreviewMessage {
  role: string;
  text: string;
}

export interface DeleteResult {
  id: string;
  tool: string;
  ok: boolean;
  error: string | null;
}
