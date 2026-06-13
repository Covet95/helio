import { create } from 'zustand';
import { tauriApi } from '@/lib/tauri';
import type { ApiProfile, StatusInfo, TargetApp } from '@/types';

interface AppStore {
  // Profiles
  profiles: ApiProfile[];
  loadingProfiles: boolean;

  // Status
  status: StatusInfo | null;
  loadingStatus: boolean;

  // Actions
  fetchProfiles: () => Promise<void>;
  addProfile: (profile: ApiProfile) => Promise<void>;
  updateProfile: (profile: ApiProfile) => Promise<void>;
  deleteProfile: (name: string) => Promise<void>;
  switchProfile: (app: TargetApp, name: string) => Promise<void>;

  fetchStatus: () => Promise<void>;

  // UI State
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
}

export const useStore = create<AppStore>((set, get) => ({
  // Initial state
  profiles: [],
  loadingProfiles: false,
  status: null,
  loadingStatus: false,
  sidebarCollapsed: false,

  // Fetch profiles
  fetchProfiles: async () => {
    set({ loadingProfiles: true });
    try {
      const profiles = await tauriApi.listProfiles();
      set({ profiles });
    } catch (error) {
      console.error('Failed to fetch profiles:', error);
    } finally {
      set({ loadingProfiles: false });
    }
  },

  // Add profile
  addProfile: async (profile) => {
    try {
      await tauriApi.addProfile(profile);
      await get().fetchProfiles();
    } catch (error) {
      console.error('Failed to add profile:', error);
      throw error;
    }
  },

  // Update profile
  updateProfile: async (profile) => {
    try {
      await tauriApi.updateProfile(profile);
      await get().fetchProfiles();
    } catch (error) {
      console.error('Failed to update profile:', error);
      throw error;
    }
  },

  // Delete profile
  deleteProfile: async (name) => {
    try {
      await tauriApi.deleteProfile(name);
      await get().fetchProfiles();
    } catch (error) {
      console.error('Failed to delete profile:', error);
      throw error;
    }
  },

  // Switch profile
  switchProfile: async (app, name) => {
    try {
      await tauriApi.switchProfile(app, name);
      await get().fetchProfiles();
      await get().fetchStatus();
    } catch (error) {
      console.error('Failed to switch profile:', error);
      throw error;
    }
  },

  // Fetch status
  fetchStatus: async () => {
    set({ loadingStatus: true });
    try {
      const status = await tauriApi.getStatus();
      set({ status });
    } catch (error) {
      console.error('Failed to fetch status:', error);
    } finally {
      set({ loadingStatus: false });
    }
  },

  // Toggle sidebar
  toggleSidebar: () => {
    set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed }));
  },
}));
