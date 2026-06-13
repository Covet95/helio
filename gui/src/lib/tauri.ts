import { invoke } from '@tauri-apps/api/core';
import type { ApiProfile, StatusInfo, TargetApp } from '@/types';

export const tauriApi = {
  // Profile 管理
  listProfiles: () =>
    invoke<ApiProfile[]>('list_profiles'),

  getProfile: (name: string) =>
    invoke<ApiProfile>('get_profile', { name }),

  addProfile: (profile: ApiProfile) =>
    invoke<number>('add_profile', { profile }),

  updateProfile: (profile: ApiProfile) =>
    invoke<void>('update_profile', { profile }),

  deleteProfile: (name: string) =>
    invoke<boolean>('delete_profile', { name }),

  switchProfile: (targetApp: TargetApp, profileName: string) =>
    invoke<void>('switch_profile', { targetApp, profileName }),

  // 配置管理
  getSharedConfig: (targetApp: TargetApp) =>
    invoke<any>('get_shared_config', { targetApp }),

  saveSharedConfig: (targetApp: TargetApp, config: any) =>
    invoke<void>('save_shared_config', { targetApp, config }),

  // 状态查询
  getStatus: () =>
    invoke<StatusInfo>('get_status'),

  // 数据库导入导出
  exportDatabase: (outputPath: string) =>
    invoke<void>('export_database', { outputPath }),

  importDatabase: (inputPath: string) =>
    invoke<void>('import_database', { inputPath }),

  // MCP 和 Skills
  scanLocalMcpServers: (targetApp: TargetApp) =>
    invoke<Record<string, any>>('scan_local_mcp_servers', { targetApp }),

  scanLocalSkills: (targetApp: TargetApp) =>
    invoke<string[]>('scan_local_skills', { targetApp }),

  getLocalConfigInfo: (targetApp: TargetApp) =>
    invoke<{
      mcp_servers: Record<string, any>;
      skills: string[];
      hooks: any;
      permissions: any;
    }>('get_local_config_info', { targetApp }),
};
