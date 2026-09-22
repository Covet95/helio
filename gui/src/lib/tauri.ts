import { invoke } from '@tauri-apps/api/core';
import type { ScannedApi } from '@/pages/importMapping';
import type {
  ApiProfile, DeleteResult, FetchedModel, LocalConfigInfo, ModelTestResult, PreviewMessage,
  SessionMeta, StatusInfo, TargetApp,
} from '@/types';

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

  assignLegacyProfile: (profileId: number, targetApp: TargetApp) =>
    command<void>('assign_legacy_profile', { profileId, targetApp }),

  deleteLegacyProfile: (profileId: number) =>
    command<boolean>('delete_legacy_profile', { profileId }),

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

  // 协议感知的模型列表加载
  fetchModels: (args: {
    targetApp: TargetApp;
    provider?: string;
    apiUrl: string;
    apiKey: string;
    envKey?: string;
    apiMode?: string;
    experimentalBearerToken?: string;
    awsProfile?: string;
    awsRegion?: string;
    hasCommandAuth?: boolean;
  }) =>
    command<FetchedModel[]>('fetch_models', { request: args }),

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
    hasCommandAuth?: boolean;
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

  exportPortableBackup: (outputPath: string) =>
    command<PortableBackupExportResult>('export_portable_backup', { outputPath }),

  importPortableBackup: (inputPath: string) =>
    command<PortableBackupImportResult>('import_portable_backup', { inputPath }),

  // 数据库自动备份的查看与回退
  listDatabaseBackups: () =>
    command<DatabaseBackupInfo[]>('list_database_backups', undefined, []),

  restoreDatabaseBackup: (backupPath: string) =>
    command<void>('restore_database_backup', { backupPath }),

  // Skills 备份/恢复
  exportSkills: (outputPath: string) =>
    command<SkillsExportResult>('export_skills', { outputPath }),

  importSkills: (inputPath: string) =>
    command<SkillsImportResult>('import_skills', { inputPath }),

  getLocalConfigInfo: (targetApp: TargetApp) =>
    command<LocalConfigInfo>('get_local_config_info', { targetApp }, {
      mcp_servers: {},
      skills: [],
      hooks: {},
      permissions: {},
      other: {},
    }),

  // 配置备份列表 / 恢复
  listConfigBackups: (targetApp: TargetApp) =>
    command<ConfigBackupInfo[]>('list_config_backups', { targetApp }, []),

  restoreConfigBackup: (targetApp: TargetApp, backupFile: string) =>
    command<string>('restore_config_backup', { targetApp, backupFile }),

  // 从本地导入
  scanLocalApi: (targetApp: TargetApp) =>
    command<ScannedApi>('scan_local_api', { targetApp }, emptyScanned(targetApp)),

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
    command<DeleteResult[]>('delete_sessions', { items }),

  cleanupSessions: (tool: string | undefined, olderThanDays: number) =>
    command<DeleteResult[]>('cleanup_sessions', { tool, olderThanDays }),
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

/** 导入/迁移前自动生成的数据库备份。 */
export interface DatabaseBackupInfo {
  path: string;
  /** 文件名里的时间戳（`YYYYmmdd_HHMMSS_ffffff`）。 */
  time: string;
  size_bytes: number;
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
  restored_names?: string[];
}

export interface PortableBackupExportResult {
  path: string;
  skills: SkillsExportResult;
}

export interface PortableBackupImportResult {
  restored_targets: string[];
  skills: SkillsImportResult;
}

/**
 * `scan_local_api` 的返回形状。
 *
 * 与 `pages/importMapping.ts` 的 `ScannedApi` 是**同一份数据**——此前两处各写
 * 一遍 29 个字段（外加 tauri.ts 里再抄一份 29 行的空值对象）。三份手工镜像
 * 意味着加一个字段要改三处，漏一处就是静默的类型漂移。
 * 这里以 `ScannedApi` 为唯一来源，另两处引用它。
 */
export type { ScannedApi } from '@/pages/importMapping';

/**
 * 非 Tauri 环境（浏览器预览）下 `scanLocalApi` 的空值。
 *
 * 只列必填字段，其余可选字段天然是 `undefined`——不必逐个写出来。
 */
function emptyScanned(targetApp: TargetApp): ScannedApi {
  return {
    found: false,
    api_url: '',
    api_key: '',
    provider: '',
    source: `${targetApp} config`,
  } as ScannedApi;
}
