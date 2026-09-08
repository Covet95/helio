import { create } from 'zustand';
import { tauriApi } from '@/lib/tauri';
import { humanizeError } from '@/lib/utils';
import type { ApiProfile, StatusInfo, TargetApp } from '@/types';
import { SUPPORTED_TOOLS } from '@/types';

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

const TOOL_IDS: ReadonlySet<string> = new Set(SUPPORTED_TOOLS.map((t) => t.id));

function readStoredTool(): TargetApp | null {
  try {
    if (typeof localStorage === 'undefined') return null;
    const saved = localStorage.getItem('helio-tool');
    if (saved && TOOL_IDS.has(saved)) return saved as TargetApp;
  } catch {
    /* 忽略持久化失败 */
  }
  return null;
}

/** 请求序号：fetch 响应只接受最新一次（后发先至的过期响应丢弃） */
let profilesSeq = 0;
let statusSeq = 0;
let profilesRequest: Promise<void> | null = null;
let statusRequest: Promise<void> | null = null;

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

  addProfile: async (profile) => {
    try {
      await tauriApi.addProfile(profile);
      await get().refresh();
    } catch (error) {
      console.error('Failed to add profile:', error);
      throw error;
    }
  },

  updateProfile: async (profile) => {
    try {
      await tauriApi.updateProfile(profile);
      await get().refresh();
    } catch (error) {
      console.error('Failed to update profile:', error);
      throw error;
    }
  },

  deleteProfile: async (targetApp, name) => {
    try {
      await tauriApi.deleteProfile(targetApp, name);
      await get().refresh();
    } catch (error) {
      console.error('Failed to delete profile:', error);
      throw error;
    }
  },

  switchProfile: async (app, name, probe) => {
    try {
      await tauriApi.switchProfile(app, name, probe);
      await get().refresh();
    } catch (error) {
      console.error('Failed to switch profile:', error);
      throw error;
    }
  },

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
    try {
      localStorage.setItem('helio-tool', tool);
    } catch {
      /* 忽略持久化失败 */
    }
    set({ selectedTool: tool });
  },
}));
