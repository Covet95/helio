import { create } from 'zustand';
import { tauriApi } from '@/lib/tauri';
import { humanizeError } from '@/lib/utils';
import type { ApiProfile, StatusInfo, TargetApp } from '@/types';

interface AppStore {
  profiles: ApiProfile[];
  loadingProfiles: boolean;
  status: StatusInfo | null;
  loadingStatus: boolean;
  /** Last user-visible global error (fetch / ops). */
  lastError: string | null;
  clearError: () => void;

  fetchProfiles: () => Promise<void>;
  addProfile: (profile: ApiProfile) => Promise<void>;
  updateProfile: (profile: ApiProfile) => Promise<void>;
  deleteProfile: (targetApp: TargetApp, name: string) => Promise<void>;
  switchProfile: (app: TargetApp, name: string) => Promise<void>;
  fetchStatus: () => Promise<void>;

  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
}

/** 请求序号：fetch 响应只接受最新一次（后发先至的过期响应丢弃） */
let profilesSeq = 0;
let statusSeq = 0;

export const useStore = create<AppStore>((set, get) => ({
  profiles: [],
  loadingProfiles: false,
  status: null,
  loadingStatus: false,
  lastError: null,
  sidebarCollapsed: false,

  clearError: () => set({ lastError: null }),

  fetchProfiles: async () => {
    // 序号守卫：只接受最新一次请求的响应，过期响应（后发先至）直接丢弃，
    // 避免快速连续操作时 UI 显示过期状态
    const seq = ++profilesSeq;
    set({ loadingProfiles: true });
    try {
      const profiles = await tauriApi.listProfiles();
      if (seq !== profilesSeq) return;
      set({ profiles, lastError: null });
    } catch (error) {
      if (seq !== profilesSeq) return;
      console.error('Failed to fetch profiles:', error);
      set({ lastError: `加载档案失败：${humanizeError(error)}` });
    } finally {
      if (seq === profilesSeq) set({ loadingProfiles: false });
    }
  },

  addProfile: async (profile) => {
    try {
      await tauriApi.addProfile(profile);
      set({ lastError: null });
      await get().fetchProfiles();
    } catch (error) {
      console.error('Failed to add profile:', error);
      throw error;
    }
  },

  updateProfile: async (profile) => {
    try {
      await tauriApi.updateProfile(profile);
      set({ lastError: null });
      await get().fetchProfiles();
      // active profile may re-apply → refresh status
      await get().fetchStatus();
    } catch (error) {
      console.error('Failed to update profile:', error);
      throw error;
    }
  },

  deleteProfile: async (targetApp, name) => {
    try {
      await tauriApi.deleteProfile(targetApp, name);
      set({ lastError: null });
      await get().fetchProfiles();
      await get().fetchStatus();
    } catch (error) {
      console.error('Failed to delete profile:', error);
      throw error;
    }
  },

  switchProfile: async (app, name) => {
    try {
      await tauriApi.switchProfile(app, name);
      set({ lastError: null });
      await get().fetchProfiles();
      await get().fetchStatus();
    } catch (error) {
      console.error('Failed to switch profile:', error);
      throw error;
    }
  },

  fetchStatus: async () => {
    const seq = ++statusSeq;
    set({ loadingStatus: true });
    try {
      const status = await tauriApi.getStatus();
      if (seq !== statusSeq) return;
      set({ status, lastError: null });
    } catch (error) {
      if (seq !== statusSeq) return;
      console.error('Failed to fetch status:', error);
      set({ lastError: `加载状态失败：${humanizeError(error)}` });
    } finally {
      if (seq === statusSeq) set({ loadingStatus: false });
    }
  },

  toggleSidebar: () => {
    set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed }));
  },
}));
