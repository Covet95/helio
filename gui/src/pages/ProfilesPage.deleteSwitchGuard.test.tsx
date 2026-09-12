// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import React from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { ApiProfile } from '../types';

const claudeDup = {
  id: 1, name: 'proxy', provider: 'anthropic',
  api_url: 'https://a.example/v1', api_key: 'sk-A', model: 'm1',
  target_app: 'claude-code', created_at: 1, updated_at: 2,
} as unknown as ApiProfile;
const codexDup = {
  id: 2, name: 'proxy', provider: 'openai',
  api_url: 'https://b.example/v1', api_key: 'sk-B', model: 'm2',
  target_app: 'codex', created_at: 1, updated_at: 2,
} as unknown as ApiProfile;
const codexApi2 = {
  id: 3, name: 'proxy-2', provider: 'openai',
  api_url: 'https://b.example/v1', api_key: 'sk-B', model: 'm2',
  target_app: 'codex', created_at: 1, updated_at: 2,
} as unknown as ApiProfile;

const api = vi.hoisted(() => ({
  listProfiles: vi.fn(), getStatus: vi.fn(), addProfile: vi.fn(),
  updateProfile: vi.fn(), deleteProfile: vi.fn(), switchProfile: vi.fn(),
  scanLocalApi: vi.fn(),
}));
vi.mock('../lib/tauri', () => ({ tauriApi: api }));

beforeEach(() => {
  vi.resetAllMocks();
  api.listProfiles.mockResolvedValue([claudeDup, codexDup, codexApi2]);
  api.getStatus.mockResolvedValue({ database: { size: 1, profile_count: 3, path: '/tmp/x' } });
  api.deleteProfile.mockResolvedValue(true);
  api.switchProfile.mockResolvedValue(undefined);
});

describe('fix A: delete captures tool', () => {
  it('deletes original tool even if dialog message shows captured tool', async () => {
    const { default: ProfilesPage } = await import('./ProfilesPage');
    const { useStore } = await import('../store');
    useStore.setState({ selectedTool: 'claude-code' as any });
    await useStore.getState().fetchProfiles(true);
    await useStore.getState().fetchStatus(true);
    render(React.createElement(MemoryRouter, null, React.createElement(ProfilesPage)));
    await waitFor(() => expect(screen.getByText('proxy')).toBeTruthy());
    // open delete on claude-code proxy
    fireEvent.click(screen.getByRole('button', { name: '删除' }));
    // dialog should mention captured tool (message contains tool label)
    await waitFor(() => expect(screen.getByText(/确定要删除/)).toBeTruthy());
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.textContent).toMatch(/Claude Code/);
    expect(dialog.textContent).toMatch(/proxy/);
    // confirm (button inside the dialog, not the card IconBtn)
    fireEvent.click(within(dialog).getByRole('button', { name: '删除' }));
    await waitFor(() => expect(api.deleteProfile).toHaveBeenCalled());
    // must be called via store deleteProfile -> tauri deleteProfile(targetApp, name)
    // store passes (targetApp, name); captured should be claude-code
    const call = (api.deleteProfile as any).mock.calls[0];
    // tauriApi.deleteProfile(targetApp, name) — first arg is tool
    expect(String(call[0])).toContain('claude');
    expect(String(call[1])).toContain('proxy');
    cleanup();
  });
});

describe('fix E: justSwitched exact match', () => {
  it('only exact name shows switched badge (proxy vs proxy-2)', async () => {
    const { default: ProfilesPage } = await import('./ProfilesPage');
    const { useStore } = await import('../store');
    useStore.setState({ selectedTool: 'codex' as any });
    await useStore.getState().fetchProfiles(true);
    await useStore.getState().fetchStatus(true);
    const { unmount } = render(React.createElement(MemoryRouter, null, React.createElement(ProfilesPage)));
    await waitFor(() => expect(screen.getByText('proxy-2')).toBeTruthy());
    const enableBtns = screen.getAllByRole('button', { name: '启用' });
    // codex tab has proxy, proxy-2 -> 2 buttons; click first (proxy)
    fireEvent.click(enableBtns[0]);
    await waitFor(() => expect(screen.getByText('已切换')).toBeTruthy());
    // must be exactly one switched badge (prefix collision would show 2)
    expect(screen.getAllByText('已切换')).toHaveLength(1);
    cleanup();
    unmount();
  });
});
