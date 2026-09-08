import { beforeEach, describe, expect, it, vi } from 'vitest';
import { deferred } from '../test/deferred';
import type { ApiProfile, StatusInfo } from '../types';

const api = vi.hoisted(() => ({
  listProfiles: vi.fn(), getStatus: vi.fn(), addProfile: vi.fn(),
}));
vi.mock('@/lib/tauri', () => ({ tauriApi: api }));

const status: StatusInfo = { database: { path: '', size: 0, profile_count: 0 } };

beforeEach(() => {
  vi.resetModules();
  vi.resetAllMocks();
});

describe('app store reads', () => {
  it('shares concurrent reads', async () => {
    const pending = deferred<ApiProfile[]>();
    api.listProfiles.mockReturnValue(pending.promise);
    const { useStore } = await import('./index');
    const first = useStore.getState().fetchProfiles();
    const second = useStore.getState().fetchProfiles();
    expect(api.listProfiles).toHaveBeenCalledTimes(1);
    expect(first).toBe(second);
    pending.resolve([]);
    await first;
    expect(useStore.getState().loadingProfiles).toBe(false);
  });

  it('does not hide a profile failure when status succeeds', async () => {
    api.listProfiles.mockRejectedValue(new Error('offline'));
    api.getStatus.mockResolvedValue(status);
    const { useStore } = await import('./index');
    await useStore.getState().fetchProfiles();
    await useStore.getState().fetchStatus();
    expect(useStore.getState().lastError).toContain('offline');
    api.listProfiles.mockResolvedValue([]);
    await useStore.getState().fetchProfiles();
    expect(useStore.getState().lastError).toBeNull();
  });

  it('ignores a stale read after a forced refresh', async () => {
    const pending = deferred<ApiProfile[]>();
    const fresh = [{ name: 'new' }] as ApiProfile[];
    api.listProfiles.mockReturnValueOnce(pending.promise).mockResolvedValueOnce(fresh);
    const { useStore } = await import('./index');
    const old = useStore.getState().fetchProfiles();
    await useStore.getState().fetchProfiles(true);
    pending.resolve([]);
    await old;
    expect(useStore.getState().profiles).toEqual(fresh);
  });

  it('refreshes the database count after adding a profile', async () => {
    api.addProfile.mockResolvedValue(1);
    api.listProfiles.mockResolvedValue([]);
    api.getStatus.mockResolvedValue(status);
    const { useStore } = await import('./index');
    await useStore.getState().addProfile({ name: 'new' } as ApiProfile);
    expect(api.getStatus).toHaveBeenCalledTimes(1);
  });
});
