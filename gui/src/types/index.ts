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

export interface OpenCodeModelLimit {
  context?: number;
  input?: number;
  output?: number;
}

export interface OpenCodeThinkingOptions {
  type?: 'enabled' | 'disabled' | string;
  budgetTokens?: number;
}

export interface OpenCodeModelOptions {
  reasoningEffort?: string;
  textVerbosity?: string;
  reasoningSummary?: string;
  include?: string[];
  thinking?: OpenCodeThinkingOptions;
  temperature?: number;
  topP?: number;
  [key: string]: unknown;
}

export interface OpenCodeVariantConfig extends OpenCodeModelOptions {
  disabled?: boolean;
}

export interface OpenCodeModelConfig {
  name?: string;
  limit?: OpenCodeModelLimit;
  options?: OpenCodeModelOptions;
  variants?: Record<string, OpenCodeVariantConfig>;
  [key: string]: unknown;
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
  /** OpenCode provider SDK mode */
  opencode_api_mode?: 'chat_completions' | 'responses' | string;
  /** OpenCode per-model config: limit/options/variants */
  model_configs?: Record<string, OpenCodeModelConfig>;
  /** Codex：写入 model_catalog.json 的模型目录（/model 列表） */
  catalog_models?: CodexCatalogModel[];
  /** 推理强度 minimal/low/medium/high/xhigh */
  reasoning_effort?: string;
  /** 推理摘要 auto/concise/detailed/none */
  reasoning_summary?: string;
  /** verbosity low/medium/high */
  verbosity?: string;
  /** 1M 上下文 */
  context_1m?: boolean;
  /** OpenClaw: models[].maxTokens（仅 OpenClaw 使用，不与 Hermes 共用语义） */
  max_tokens?: number;
  /** Codex provider wire protocol, e.g. responses or chat. */
  wire_api?: string;
  /** Codex provider-scoped API key environment variable. */
  env_key?: string;
  /** Codex provider authentication mode. */
  requires_openai_auth?: boolean;
  /** Codex provider-specific bearer token. */
  experimental_bearer_token?: string;
  /** Codex 顶层 service_tier：fast（legacy）/ flex / priority */
  service_tier?: string;
  /** Codex auth 命令（[model_providers.<id>.auth].command），与 env_key/bearer 互斥 */
  auth_command?: string;
  /** Codex auth 命令参数 */
  auth_args?: string[];
  /** Codex auth 超时毫秒数 */
  auth_timeout_ms?: number;
  /** Codex auth token 刷新间隔毫秒数 */
  auth_refresh_interval_ms?: number;
  /** Codex auth 命令工作目录（高级，多数留空） */
  auth_cwd?: string;
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
  /** 归属工具；旧数据库可能为 undefined，但新建/更新时必须填写。 */
  target_app?: TargetApp;
  created_at?: number;
  updated_at?: number;
}

export interface FetchedModel {
  id: string;
  owned_by?: string;
  display_name?: string;
  context_window?: number;
  capabilities?: string[];
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

export type TargetApp = 'claude-code' | 'codex' | 'pi' | 'opencode' | 'hermes' | 'openclaw' | 'zcode';

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
  { id: 'zcode', displayName: 'ZCode', short: 'ZC', color: '#2563EB', format: 'JSON' },
];

export function toolById(id: TargetApp | string): ToolInfo | undefined {
  return SUPPORTED_TOOLS.find((t) => t.id === id);
}

export interface TargetStatus {
  profile?: ApiProfile;
  connected: boolean;
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
  /** Provider is managed by the target tool and has no probe URL. */
  managed?: boolean;
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
  zcode?: TargetStatus;
  database: DatabaseInfo;
}

/** 单个 MCP server 的配置（只读展示）。 */
export interface McpServerConfig {
  command?: string;
  args?: string[];
  url?: string | null;
  env?: Record<string, string> | null;
}

/**
 * `get_local_config_info` 的返回体：某工具当前 live 配置里被同步的部分。
 *
 * 字段与后端 `LocalConfigInfo` 一一对应——`other` 曾在此缺失，页面只好用
 * `as LocalInfo` 强转掩盖，导致类型体系失效（后端新增字段前端不会报错）。
 */
export interface LocalConfigInfo {
  mcp_servers: Record<string, McpServerConfig>;
  skills: string[];
  hooks: Record<string, unknown>;
  permissions: Record<string, unknown>;
  /** 其余被同步但未单独归类的顶层配置（tui / plugins / features 等）。 */
  other: Record<string, unknown>;
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

/**
 * 命令层错误类别。与 Rust `switch_api::error::ErrorKind` 一一对应,
 * 由 `tests/frontend_types_sync.rs` 守卫。
 *
 * 请按 `kind` 分支,不要匹配 `message` 文案——文案随时可能调整。
 */
export type ErrorKind =
  | 'not_found'
  | 'invalid_input'
  | 'permission'
  | 'conflict'
  | 'io'
  | 'partial_failure'
  | 'internal';

/**
 * 命令层结构化错误。
 *
 * 尚未迁移的命令仍返回字符串,`toUserMessage()` 两种形状都接受,
 * 因此新旧命令可以长期共存。
 */
export interface AppError {
  kind: ErrorKind;
  message: string;
  detail?: string;
}
