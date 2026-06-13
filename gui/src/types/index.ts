export interface ApiProfile {
  id?: number;
  name: string;
  provider: string;
  api_url: string;
  api_key: string;
  model_mapping?: Record<string, string>;
  created_at?: number;
  updated_at?: number;
}

export type TargetApp = 'claude-code' | 'codex' | 'gemini' | 'opencode';

/// 已注册工具的元数据，用于动态生成 UI
export interface ToolInfo {
  id: TargetApp;
  displayName: string;
}

export const SUPPORTED_TOOLS: ToolInfo[] = [
  { id: 'claude-code', displayName: 'Claude Code' },
  { id: 'codex', displayName: 'Codex' },
  { id: 'gemini', displayName: 'Gemini CLI' },
  { id: 'opencode', displayName: 'OpenCode' },
];

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

export interface SharedConfig {
  [key: string]: any;
}
