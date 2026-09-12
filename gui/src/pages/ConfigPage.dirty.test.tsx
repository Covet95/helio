// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor, cleanup } from '@testing-library/react';
import React from 'react';
import { MemoryRouter } from 'react-router-dom';

const api = vi.hoisted(() => ({
  getLocalConfigInfo: vi.fn(), listConfigBackups: vi.fn(),
  updateCodexFields: vi.fn(), readCodexConfigRaw: vi.fn(),
}));
vi.mock('../lib/tauri', () => ({ tauriApi: api }));

beforeEach(() => {
  vi.resetAllMocks();
  api.getLocalConfigInfo.mockResolvedValue({
    mcp_servers: {}, skills: [], hooks: {}, permissions: {}, other: { approval_policy: 'untrusted' },
  });
  api.listConfigBackups.mockResolvedValue([]);
});

describe('fix D: codex behavior keeps dirty edits across refresh', () => {
  it('preserves unsaved select after background reload', async () => {
    const { useStore } = await import('../store');
    useStore.setState({ selectedTool: 'codex' as any });
    const { default: ConfigPage } = await import('./ConfigPage');
    render(React.createElement(MemoryRouter, null, React.createElement(ConfigPage)));
    const sel = (await screen.findByLabelText('approval_policy')) as HTMLSelectElement;
    await waitFor(() => expect(sel.value).toBe('untrusted'));
    fireEvent.change(sel, { target: { value: 'never' } });
    expect(sel.value).toBe('never');
    // trigger background reload with same server values (header refresh is first)
    const refreshBtns = screen.getAllByRole('button', { name: /刷新/ });
    fireEvent.click(refreshBtns[0]);
    await waitFor(() => expect(api.getLocalConfigInfo).toHaveBeenCalledTimes(2));
    // dirty edit must survive (old code would reset to untrusted)
    await waitFor(() => expect((screen.getByLabelText('approval_policy') as HTMLSelectElement).value).toBe('never'));
    cleanup();
  });
});
