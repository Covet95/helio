import { create } from 'zustand';
import { tauriApi } from '@/lib/tauri';
import { humanizeError } from '@/lib/utils';
import { readSelectedTool, writeSelectedTool } from '@/lib/settings';
import type { ApiProfile, StatusInfo, TargetApp } from '@/types';

interface AppStore {
  profiles: ApiProfile[];
  loadingProfiles: boolean;
  status: StatusInfo | null;
  loadingStatus: boolean;
  /** Last user-visible global error (fetch / ops). */
  lastError: string | null;
  profilesError: string | null;
  statusError: string | null;
  clearError: () => void;

  fetchProfiles: (force?: boolean) => Promise<void>;
  addProfile: (profile: ApiProfile) => Promise<void>;
  updateProfile: (profile: ApiProfile) => Promise<void>;
  deleteProfile: (targetApp: TargetApp, name: string) => Promise<void>;
  switchProfile: (app: TargetApp, name: string, probe?: boolean) => Promise<void>;
  fetchStatus: (force?: boolean) => Promise<void>;
  refresh: () => Promise<void>;

  sidebarCollapsed: boolean;
  toggleSidebar: () => void;

  /** 当前所选工具：档案 / 共享配置 / 导入三页共用，跨页保持。 */
  selectedTool: TargetApp;
  setSelectedTool: (tool: TargetApp) => void;
}

function readStoredTool(): TargetApp | null {
  return readSelectedTool();
}

/** 请求序号：fetch 响应只接受最新一次（后发先至的过期响应丢弃） */
let profilesSeq = 0;
let statusSeq = 0;
let profilesRequest: Promise<void> | null = null;
let statusRequest: Promise<void> | null = null;

/**
 * 写操作 → 刷新，失败时记日志并**原样抛出**。
 *
 * 四个 mutation 原先各写一遍同样的 try/catch/refresh（只差日志文案）。
 * 关键是必须把错误继续抛给调用方——页面据此显示「启用失败：<原因>」，
 * 在这里吞掉会让失败静默。
 */
async function runProfileMutation(label: string, op: () => Promise<unknown>): Promise<void> {
  try {
    await op();
    await useStore.getState().refresh();
  } catch (error) {
    console.error(`Failed to ${label}:`, error);
    throw error;
  }
}

export const useStore = create<AppStore>((set, get) => ({
  profiles: [],
  loadingProfiles: false,
  status: null,
  loadingStatus: false,
  lastError: null,
  profilesError: null,
  statusError: null,
  sidebarCollapsed: false,
  selectedTool: readStoredTool() ?? 'claude-code',

  clearError: () => set({ lastError: null, profilesError: null, statusError: null }),

  fetchProfiles: (force = false) => {
    if (profilesRequest && !force) return profilesRequest;
    const seq = ++profilesSeq;
    set({ loadingProfiles: true });
    profilesRequest = (async () => {
      try {
        const profiles = await tauriApi.listProfiles();
        if (seq !== profilesSeq) return;
        set((state) => ({ profiles, profilesError: null, lastError: state.statusError }));
      } catch (error) {
        if (seq !== profilesSeq) return;
        const message = `加载档案失败：${humanizeError(error)}`;
        set({ profilesError: message, lastError: message });
      } finally {
        if (seq === profilesSeq) {
          profilesRequest = null;
          set({ loadingProfiles: false });
        }
      }
    })();
    return profilesRequest;
  },

  refresh: async () => {
    await Promise.all([get().fetchProfiles(true), get().fetchStatus(true)]);
  },

  addProfile: (profile) => runProfileMutation('add profile', () => tauriApi.addProfile(profile)),
  updateProfile: (profile) => runProfileMutation('update profile', () => tauriApi.updateProfile(profile)),
  deleteProfile: (targetApp, name) =>
    runProfileMutation('delete profile', () => tauriApi.deleteProfile(targetApp, name)),
  switchProfile: (app, name, probe) =>
    runProfileMutation('switch profile', () => tauriApi.switchProfile(app, name, probe)),

  fetchStatus: (force = false) => {
    if (statusRequest && !force) return statusRequest;
    const seq = ++statusSeq;
    set({ loadingStatus: true });
    statusRequest = (async () => {
      try {
        const status = await tauriApi.getStatus();
        if (seq !== statusSeq) return;
        set((state) => ({ status, statusError: null, lastError: state.profilesError }));
      } catch (error) {
        if (seq !== statusSeq) return;
        const message = `加载状态失败：${humanizeError(error)}`;
        set({ statusError: message, lastError: message });
      } finally {
        if (seq === statusSeq) {
          statusRequest = null;
          set({ loadingStatus: false });
        }
      }
    })();
    return statusRequest;
  },

  toggleSidebar: () => {
    set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed }));
  },

  setSelectedTool: (tool) => {
    writeSelectedTool(tool);
    set({ selectedTool: tool });
  },
}));
