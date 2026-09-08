// @vitest-environment jsdom
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { ApiProfile } from '../types';

const mks = {
  id: 20,
  name: 'mks',
  provider: 'openai',
  api_url: 'https://cn2.picpi.top/v1',
  api_key: 'sk-TESTKEY',
  model: 'gpt-5.6-sol',
  reasoning_effort: 'medium',
  context_1m: true,
  target_app: 'codex',
  wire_api: 'responses',
  api_keys: [
    { id: 'k1', label: 'default', key: 'sk-TESTKEY', is_active: true, created_at: 1784188509 },
  ],
  created_at: 1781451144,
  updated_at: 1785810841,
} as unknown as ApiProfile;

vi.mock('../lib/tauri', () => ({
  tauriApi: {
    listProfiles: vi.fn().mockResolvedValue([mks]),
    getStatus: vi.fn().mockResolvedValue({
      codex: { profile: mks, connected: true },
      database: { size: 1, profile_count: 1, path: '/tmp/x' },
    }),
    addProfile: vi.fn(),
    updateProfile: vi.fn(),
    deleteProfile: vi.fn(),
    switchProfile: vi.fn(),
    scanLocalApi: vi.fn(),
  },
}));

beforeAll(() => {
  if (!HTMLDialogElement.prototype.showModal) {
    HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
      this.setAttribute('open', '');
    };
    HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
      this.removeAttribute('open');
    };
  }
});

describe('ProfilesPage edit wiring', () => {
  it('opens the edit modal with api fields', async () => {
    const { default: ProfilesPage } = await import('./ProfilesPage');
    const { useStore } = await import('../store');
    await useStore.getState().fetchProfiles();
    await useStore.getState().fetchStatus();
    render(React.createElement(MemoryRouter, null, React.createElement(ProfilesPage)));
    // switch to the codex tab (store defaults to claude-code)
    fireEvent.click(screen.getByRole('button', { name: 'Codex' }));
    await waitFor(() => expect(screen.getByText('mks')).toBeTruthy());
    await waitFor(() => expect(screen.getByText('https://cn2.picpi.top/v1')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: '编辑' }));
    await waitFor(() => expect(screen.getByLabelText('API URL')).toBeTruthy());
    expect(screen.getByLabelText('API Key')).toBeTruthy();
    expect(screen.getByDisplayValue('https://cn2.picpi.top/v1')).toBeTruthy();
    cleanup();
  });
});
