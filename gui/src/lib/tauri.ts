import { invoke } from '@tauri-apps/api/core';
import type { ApiProfile, FetchedModel, ModelTestResult, StatusInfo, TargetApp, SessionMeta, PreviewMessage, DeleteResult } from '@/types';

const canUseTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function command<T>(name: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (!canUseTauri()) {
    if (fallback !== undefined) return Promise.resolve(fallback);
    return Promise.reject(new Error('请在 Helio 桌面应用内执行此操作'));
  }
  return invoke<T>(name, args);
}

const emptyStatus: StatusInfo = {
  database: {
    size: 0,
    profile_count: 0,
    path: '',
  },
};

export const tauriApi = {
  // Profile 管理
  listProfiles: () =>
    command<ApiProfile[]>('list_profiles', undefined, []),

  addProfile: (profile: ApiProfile) =>
    command<number>('add_profile', { profile }),

  updateProfile: (profile: ApiProfile) =>
    command<void>('update_profile', { profile }),

  deleteProfile: (targetApp: TargetApp, name: string) =>
    command<boolean>('delete_profile', { name, targetApp }),

  switchProfile: (targetApp: TargetApp, profileName: string) =>
    command<void>('switch_profile', { targetApp, profileName }),

  copyText: (text: string) =>
    command<void>('copy_text', { text }),

  // 模型列表加载（OpenAI 兼容 /v1/models）
  fetchModels: (apiUrl: string, apiKey: string) =>
    command<FetchedModel[]>('fetch_models', { apiUrl, apiKey }),

  testModel: (apiUrl: string, apiKey: string, model: string, wireApi?: string) =>
    command<ModelTestResult>('test_model', { apiUrl, apiKey, model, wireApi }),

  // 配置管理

  // 状态查询
  getStatus: () =>
    command<StatusInfo>('get_status', undefined, emptyStatus),

  // 数据库导入导出
  exportDatabase: (outputPath: string) =>
    command<void>('export_database', { outputPath }),

  importDatabase: (inputPath: string) =>
    command<void>('import_database', { inputPath }),

  getLocalConfigInfo: (targetApp: TargetApp) =>
    command<{
      mcp_servers: Record<string, any>;
      skills: string[];
      hooks: any;
      permissions: any;
    }>('get_local_config_info', { targetApp }, {
      mcp_servers: {},
      skills: [],
      hooks: {},
      permissions: {},
    }),

  // 从本地导入
  scanLocalApi: (targetApp: TargetApp) =>
    command<{
      found: boolean;
      api_url: string;
      api_key: string;
      provider: string;
      model?: string;
      model_mapping?: Record<string, string>;
      reasoning_effort?: string;
      context_1m?: boolean;
      wire_api?: string;
      requires_openai_auth?: boolean;
      experimental_bearer_token?: string;
      model_thinking_enabled?: boolean;
      service_tier?: string;
      api_mode?: string;
      max_tokens?: number;
      source: string;
    }>('scan_local_api', { targetApp }, {
      found: false,
      api_url: '',
      api_key: '',
      provider: '',
      model: undefined,
      model_mapping: undefined,
      reasoning_effort: undefined,
      context_1m: undefined,
      wire_api: undefined,
      requires_openai_auth: undefined,
      experimental_bearer_token: undefined,
      model_thinking_enabled: undefined,
      service_tier: undefined,
      api_mode: undefined,
      max_tokens: undefined,
      source: `${targetApp} config`,
    }),

  importSharedConfig: (targetApp: TargetApp) =>
    command<any>('import_shared_config', { targetApp }),

  // Codex config.toml 原始文本编辑（仅 Codex）
  readCodexConfigRaw: () =>
    command<string>('read_codex_config_raw', undefined, ''),

  saveCodexConfigRaw: (content: string) =>
    command<void>('save_codex_config_raw', { content }),

  // 编辑 Codex 全局行为字段（顶层键），写回 ~/.codex/config.toml。
  // null 值表示删除该字段。
  updateCodexFields: (fields: Record<string, unknown>) =>
    command<void>('update_codex_fields', { fields }),

  // 从 cc-switch 导入
  scanCcSwitch: (targetApp: string) =>
    command<CcSwitchProvider[]>('scan_cc_switch', { targetApp }, []),

  importCcSwitch: (targetApp: string, providers: CcSwitchProvider[]) =>
    command<number>('import_cc_switch', { targetApp, providers }),

  // 会话历史
  listSessions: (tool?: string, search?: string) =>
    command<SessionMeta[]>('list_sessions', { tool, search }, []),

  readSessionPreview: (tool: string, id: string) =>
    command<PreviewMessage[]>('read_session_preview', { tool, id }, []),

  deleteSession: (tool: string, id: string) =>
    command<DeleteResult>('delete_session', { tool, id }),

  deleteSessions: (items: { tool: string; id: string }[]) =>
    command<DeleteResult[]>('delete_sessions', { items }, []),

  cleanupSessions: (tool: string | undefined, olderThanDays: number) =>
    command<DeleteResult[]>('cleanup_sessions', { tool, olderThanDays }, []),
};

export interface CcSwitchProvider {
  name: string;
  app_type: string;
  api_url: string;
  api_key: string;
  provider: string;
  model?: string;
  model_mapping?: Record<string, string>;
  reasoning_effort?: string;
  context_1m: boolean;
  wire_api?: string;
  requires_openai_auth?: boolean;
  experimental_bearer_token?: string;
  model_thinking_enabled?: boolean;
  service_tier?: string;
  is_current: boolean;
}
