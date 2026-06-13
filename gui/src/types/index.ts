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

export type TargetApp = 'claude-code' | 'codex';

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
  database: DatabaseInfo;
}

export interface SharedConfig {
  [key: string]: any;
}
