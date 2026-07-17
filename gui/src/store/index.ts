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

export const useStore = create<AppStore>((set, get) => ({
  profiles: [],
  loadingProfiles: false,
  status: null,
  loadingStatus: false,
  lastError: null,
  sidebarCollapsed: false,

  clearError: () => set({ lastError: null }),

  fetchProfiles: async () => {
    set({ loadingProfiles: true });
    try {
      const profiles = await tauriApi.listProfiles();
      set({ profiles, lastError: null });
    } catch (error) {
      console.error('Failed to fetch profiles:', error);
      set({ lastError: `加载档案失败：${humanizeError(error)}` });
    } finally {
      set({ loadingProfiles: false });
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
    set({ loadingStatus: true });
    try {
      const status = await tauriApi.getStatus();
      set({ status, lastError: null });
    } catch (error) {
      console.error('Failed to fetch status:', error);
      set({ lastError: `加载状态失败：${humanizeError(error)}` });
    } finally {
      set({ loadingStatus: false });
    }
  },

  toggleSidebar: () => {
    set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed }));
  },
}));
