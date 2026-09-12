// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor, cleanup } from '@testing-library/react';
import React from 'react';
import type { ApiProfile } from '../../types';

const api = vi.hoisted(() => ({
  fetchModels: vi.fn(), testModel: vi.fn(), failoverProfileKeys: vi.fn(),
}));
vi.mock('../../lib/tauri', () => ({ tauriApi: api }));

const base: ApiProfile = {
  id: 9, name: 'saved-name', provider: 'openai',
  api_url: 'https://api.example/v1', api_key: 'sk-AAA', model: 'm1',
  target_app: 'codex',
  api_keys: [
    { id: 'k1', label: 'a', key: 'sk-AAA', is_active: true },
    { id: 'k2', label: 'b', key: 'sk-BBB', is_active: false },
  ],
} as unknown as ApiProfile;

beforeEach(() => {
  vi.resetAllMocks();
  api.failoverProfileKeys.mockResolvedValue({ success: true, active_key_id: 'k2', active_label: 'b', tried: [], re_switched: false });
});

describe('fix B: failover uses saved name', () => {
  it('blocks failover when name is dirty and uses saved name otherwise', async () => {
    const { ProfileModal } = await import('./ProfileFormModal');
    render(React.createElement(ProfileModal, {
      profile: base, initialTool: 'codex' as any,
      onClose: () => {}, onSave: async () => {},
    }));
    // Failover button visible (multi-key edit mode)
    const btn = await screen.findByRole('button', { name: 'Failover' });
    // rename in form (unsaved)
    const nameInput = screen.getByLabelText('名称') as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: 'hacked-name' } });
    fireEvent.click(btn);
    await waitFor(() => expect(screen.getByText(/请先保存后再 failover/)).toBeTruthy());
    expect(api.failoverProfileKeys).not.toHaveBeenCalled();
    // revert to saved name -> should call backend with saved name
    fireEvent.change(nameInput, { target: { value: 'saved-name' } });
    fireEvent.click(btn);
    await waitFor(() => expect(api.failoverProfileKeys).toHaveBeenCalledTimes(1));
    expect((api.failoverProfileKeys as any).mock.calls[0][1]).toBe('saved-name');
    cleanup();
  });
});
