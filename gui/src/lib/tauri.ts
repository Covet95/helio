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

  switchProfile: (targetApp: TargetApp, profileName: string, probe?: boolean) =>
    command<void>('switch_profile', { targetApp, profileName, probe: !!probe }),

  failoverProfileKeys: (targetApp: TargetApp | string, profileName: string, reSwitch?: boolean) =>
    command<{
      success: boolean;
      active_key_id?: string;
      active_label?: string;
      tried: Array<{
        key_id: string;
        label: string;
        ok: boolean;
        error?: string;
        endpoint?: string;
        protocol?: string;
      }>;
      re_switched: boolean;
    }>('failover_profile_keys', {
      targetApp,
      profileName,
      reSwitch,
    }),

  probeActiveProfiles: () =>
    command<import('@/types').ToolProbeResult[]>('probe_active_profiles'),

  copyText: (text: string) =>
    command<void>('copy_text', { text }),

  // 模型列表加载（OpenAI 兼容 /v1/models）
  fetchModels: (apiUrl: string, apiKey: string) =>
    command<FetchedModel[]>('fetch_models', { apiUrl, apiKey }),

  testModel: (args: {
    targetApp: TargetApp | string;
    apiUrl: string;
    apiKey: string;
    model: string;
    envKey?: string;
    wireApi?: string;
    apiMode?: string;
    experimentalBearerToken?: string;
    keyLabel?: string;
  }) =>
    command<ModelTestResult>('test_model', {
      request: args,
    }),

  // 配置管理

  // 状态查询
  getStatus: () =>
    command<StatusInfo>('get_status', undefined, emptyStatus),

  // 数据库导入导出
  exportDatabase: (outputPath: string) =>
    command<void>('export_database', { outputPath }),

  importDatabase: (inputPath: string) =>
    command<void>('import_database', { inputPath }),

  // Skills 备份/恢复
  exportSkills: (outputPath: string) =>
    command<SkillsExportResult>('export_skills', { outputPath }, {
      apps: [],
      total: 0,
      path: '',
    }),

  importSkills: (inputPath: string) =>
    command<SkillsImportResult>('import_skills', { inputPath }),

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

  // 配置备份列表 / 恢复
  listConfigBackups: (targetApp: TargetApp) =>
    command<ConfigBackupInfo[]>('list_config_backups', { targetApp }, []),

  restoreConfigBackup: (targetApp: TargetApp, backupFile: string) =>
    command<string>('restore_config_backup', { targetApp, backupFile }),

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
      env_key?: string;
      requires_openai_auth?: boolean;
      experimental_bearer_token?: string;
      service_tier?: string;
      supports_standalone_web_search?: boolean;
      aws_profile?: string;
      aws_region?: string;
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
      env_key: undefined,
      requires_openai_auth: undefined,
      experimental_bearer_token: undefined,
      service_tier: undefined,
      supports_standalone_web_search: undefined,
      aws_profile: undefined,
      aws_region: undefined,
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
  env_key?: string;
  requires_openai_auth?: boolean;
  experimental_bearer_token?: string;
  service_tier?: string;
  is_current: boolean;
}

export interface ConfigBackupInfo {
  path: string;
  time: string;
  target: string | null;
}

export interface SkillsExportResult {
  apps: { app: string; count: number }[];
  total: number;
  path: string;
}

export interface SkillsImportResult {
  restored: number;
  skipped: number;
  skipped_names: string[];
}
