export interface ApiProfile {
  id?: number;
  name: string;
  provider: string;
  api_url: string;
  api_key: string;
  model_mapping?: Record<string, string>;
  /** 默认模型 */
  model?: string;
  /** 推理强度 low/medium/high/xhigh */
  reasoning_effort?: string;
  /** 1M 上下文 */
  context_1m?: boolean;
  /** 归属工具；undefined = 通用（所有工具下都显示）*/
  target_app?: TargetApp;
  created_at?: number;
  updated_at?: number;
}

export interface FetchedModel {
  id: string;
  owned_by?: string;
}

export type TargetApp = 'claude-code' | 'codex' | 'gemini' | 'opencode';

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
  { id: 'gemini', displayName: 'Gemini CLI', short: 'GM', color: '#4F8DF6', format: '.env' },
  { id: 'opencode', displayName: 'OpenCode', short: 'OC', color: '#4B5563', format: 'JSON' },
];

export function toolById(id: TargetApp | string): ToolInfo | undefined {
  return SUPPORTED_TOOLS.find((t) => t.id === id);
}

export interface TargetStatus {
  profile?: ApiProfile;
  connected: boolean;
  latency?: number;
}

export interface DatabaseInfo {
  size: number;
  profile_count: number;
  path: string;
}

export interface StatusInfo {
  claude_code?: TargetStatus;
  codex?: TargetStatus;
  gemini?: TargetStatus;
  opencode?: TargetStatus;
  database: DatabaseInfo;
}
