import { invoke } from '@tauri-apps/api/core';
import type { ApiProfile, FetchedModel, StatusInfo, TargetApp } from '@/types';

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

  getProfile: (name: string) =>
    command<ApiProfile>('get_profile', { name }),

  addProfile: (profile: ApiProfile) =>
    command<number>('add_profile', { profile }),

  updateProfile: (profile: ApiProfile) =>
    command<void>('update_profile', { profile }),

  deleteProfile: (name: string) =>
    command<boolean>('delete_profile', { name }),

  switchProfile: (targetApp: TargetApp, profileName: string) =>
    command<void>('switch_profile', { targetApp, profileName }),

  // 模型列表加载（OpenAI 兼容 /v1/models）
  fetchModels: (apiUrl: string, apiKey: string) =>
    command<FetchedModel[]>('fetch_models', { apiUrl, apiKey }),

  // 配置管理

  // 状态查询
  getStatus: () =>
    command<StatusInfo>('get_status', undefined, emptyStatus),

  // 数据库导入导出
  exportDatabase: (outputPath: string) =>
    command<void>('export_database', { outputPath }),

  importDatabase: (inputPath: string) =>
    command<void>('import_database', { inputPath }),

  // MCP 和 Skills
  scanLocalMcpServers: (targetApp: TargetApp) =>
    command<Record<string, any>>('scan_local_mcp_servers', { targetApp }, {}),

  scanLocalSkills: (targetApp: TargetApp) =>
    command<string[]>('scan_local_skills', { targetApp }, []),

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
      source: string;
    }>('scan_local_api', { targetApp }, {
      found: false,
      api_url: '',
      api_key: '',
      provider: '',
      source: `${targetApp} config`,
    }),

  importSharedConfig: (targetApp: TargetApp) =>
    command<any>('import_shared_config', { targetApp }),

  // 从 cc-switch 导入
  scanCcSwitch: (targetApp: string) =>
    command<CcSwitchProvider[]>('scan_cc_switch', { targetApp }, []),

  importCcSwitch: (targetApp: string, providers: CcSwitchProvider[]) =>
    command<number>('import_cc_switch', { targetApp, providers }),
};

export interface CcSwitchProvider {
  name: string;
  app_type: string;
  api_url: string;
  api_key: string;
  provider: string;
  model?: string;
  reasoning_effort?: string;
  context_1m: boolean;
  is_current: boolean;
}
